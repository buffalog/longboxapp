//! Library Tidy — duplicate physical files.
//!
//! An issue with more than one `is_present = 1` file has multiple physical
//! copies of the same comic (split across folders, or same folder under
//! different names/formats). This surfaces them and lets a human keep one
//! copy and permanently delete the rest — file on disk AND catalog row.
//!
//! Two safety rails, because a bug here deletes real user files:
//!
//! 1. **Mismatch exclusion.** Some issues have >1 present file not because of
//!    duplication but because distinct issues were wrongly matched to one
//!    issue row (e.g. files for #1, #2, #5 all under one `issue_id`). Those
//!    are NOT duplicates — deleting the "losers" would destroy distinct
//!    issues. Every candidate's own issue number is parsed (filename, then
//!    cached ComicInfo); a group is actionable only when they all agree.
//!    Mismatch groups are surfaced read-only (`kind = "mismatch"`) and the
//!    resolve endpoint independently re-validates and refuses them.
//!
//! 2. **Server never picks.** The suggested keep is only a hint in the GET
//!    payload; the resolve endpoint acts solely on explicit
//!    `{issue_id, keep_file_id}` pairs the client sends. "Resolve all with
//!    defaults" is a client action that pre-fills those pairs.
//!
//! Resolution order per losing file: delete the physical file first
//! (already-missing counts as success), then hard-delete the DB row. If the
//! file delete fails the row is left intact (retryable); the kept file/row is
//! never touched.

use std::collections::HashMap;
use std::path::Path;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_core::{parse_filename, ComicInfo, ParsingPattern};
use longbox_db::{
    file_repo, library_root_repo, parsing_pattern_repo, DuplicateFileCandidate, FileRow,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::pathsafe::is_contained;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/library/tidy/duplicate-files", get(list))
        .route("/library/tidy/duplicate-files/resolve", post(resolve))
}

/// A candidate whose size is below this absolute floor is treated as
/// possibly-corrupt and never suggested as the keep. Real comics are
/// megabytes; sub-1-MiB archives are almost always truncated/corrupt.
const SUSPECT_ABS_FLOOR_BYTES: i64 = 1_048_576;
/// …and so is a candidate smaller than this fraction (1/N) of its largest
/// sibling — catches a 0.6 MB copy next to a healthy 90 MB one.
// ponytail: fixed floor + ratio; expose as settings if libraries vary enough
// to need tuning.
const SUSPECT_RATIO_DENOM: i64 = 10;

const DEFAULT_PER_PAGE: i64 = 50;
const MAX_PER_PAGE: i64 = 200;

// -------- GET: list groups --------

#[derive(Debug, Deserialize)]
struct ListParams {
    #[serde(default)]
    page: Option<i64>,
    #[serde(default)]
    per_page: Option<i64>,
}

#[derive(Debug, Serialize, PartialEq)]
struct DupCandidate {
    file_id: i64,
    path_relative: String,
    size_bytes: i64,
    /// `cbz` | `cbr` | `cb7` | `other`, from the path extension.
    format: String,
    /// The issue number parsed from this file (filename, then cached
    /// ComicInfo). `None` when neither yields a number.
    parsed_number: Option<String>,
    /// The copy the app currently serves (owned-preferred, then lowest id).
    is_served: bool,
    /// Size looks corrupt relative to the floor / its siblings — flagged so a
    /// reviewer's eye catches it, and never auto-suggested as the keep.
    suspect_corrupt: bool,
    /// Lives under a `_unsorted/` staging path (less canonical).
    under_unsorted: bool,
}

#[derive(Debug, Serialize, PartialEq)]
struct DupGroup {
    issue_id: i64,
    series_title: String,
    issue_number: String,
    /// `duplicate` (all candidates agree on issue number → actionable) or
    /// `mismatch` (numbers disagree → distinct issues wrongly merged; NOT
    /// deletable here, needs a re-split fix instead).
    kind: &'static str,
    /// Pre-selected keep — a hint only; `Some` only for `duplicate` groups.
    suggested_keep_file_id: Option<i64>,
    files: Vec<DupCandidate>,
}

#[derive(Debug, Serialize)]
struct ListResponse {
    groups: Vec<DupGroup>,
    total: i64,
    page: i64,
    per_page: i64,
}

async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ListResponse>, ApiError> {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let offset = (page - 1) * per_page;

    let total = file_repo::count_duplicate_file_groups(&state.db).await?;
    let candidates = file_repo::duplicate_file_candidates_page(&state.db, per_page, offset).await?;
    let patterns = load_patterns(&state).await?;

    let groups = build_groups(candidates, &patterns);
    Ok(Json(ListResponse {
        groups,
        total,
        page,
        per_page,
    }))
}

/// Group the (issue_id, id)-ordered candidate rows into per-issue groups and
/// annotate each: parsed number, format, served flag, suspect-size flag, and
/// the group's kind + keep-suggestion. Pure over its inputs so it's testable
/// without a DB.
fn build_groups(
    candidates: Vec<DuplicateFileCandidate>,
    patterns: &[ParsingPattern],
) -> Vec<DupGroup> {
    let mut groups: Vec<DupGroup> = Vec::new();
    // Rows arrive contiguous per issue_id (repo ORDER BY issue_id, id).
    let mut i = 0;
    while i < candidates.len() {
        let issue_id = candidates[i].issue_id;
        let mut j = i;
        while j < candidates.len() && candidates[j].issue_id == issue_id {
            j += 1;
        }
        let rows = &candidates[i..j];
        groups.push(build_one_group(rows, patterns));
        i = j;
    }
    groups
}

fn build_one_group(rows: &[DuplicateFileCandidate], patterns: &[ParsingPattern]) -> DupGroup {
    let largest = rows.iter().map(|r| r.size_bytes).max().unwrap_or(0);
    // Served pick: owned-preferred, then lowest id — mirrors opds_repo.
    let served_id = rows
        .iter()
        .min_by_key(|r| (r.status != "owned", r.file_id))
        .map(|r| r.file_id);

    let files: Vec<DupCandidate> = rows
        .iter()
        .map(|r| DupCandidate {
            file_id: r.file_id,
            path_relative: r.path_relative.clone(),
            size_bytes: r.size_bytes,
            format: derive_format(&r.path_relative).to_owned(),
            parsed_number: parse_candidate_number(
                &r.path_relative,
                r.cached_comicinfo_xml.as_deref(),
                patterns,
            ),
            is_served: Some(r.file_id) == served_id,
            suspect_corrupt: is_suspect_size(r.size_bytes, largest),
            under_unsorted: under_unsorted(&r.path_relative),
        })
        .collect();

    let numbers: Vec<Option<String>> = files.iter().map(|f| f.parsed_number.clone()).collect();
    let is_dup = classify_is_duplicate(&numbers);
    let suggested_keep_file_id = if is_dup { suggest_keep(&files) } else { None };

    DupGroup {
        issue_id: rows[0].issue_id,
        series_title: rows[0].series_title.clone(),
        issue_number: rows[0].issue_number.clone(),
        kind: if is_dup { "duplicate" } else { "mismatch" },
        suggested_keep_file_id,
        files,
    }
}

// -------- POST: resolve --------

#[derive(Debug, Deserialize)]
struct ResolveBody {
    resolutions: Vec<Resolution>,
}

#[derive(Debug, Deserialize)]
struct Resolution {
    issue_id: i64,
    keep_file_id: i64,
}

#[derive(Debug, Serialize)]
struct ResolveResult {
    issue_id: i64,
    /// `resolved` (losers deleted) or `refused` (safety check failed — see
    /// `reason`; nothing was touched).
    status: &'static str,
    kept_file_id: Option<i64>,
    deleted_file_ids: Vec<i64>,
    /// Losers whose physical delete failed — their DB rows were left intact.
    failed: Vec<ResolveFailure>,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResolveFailure {
    file_id: i64,
    error: String,
}

#[derive(Debug, Serialize)]
struct ResolveResponse {
    results: Vec<ResolveResult>,
}

/// Serializes all resolve batches process-wide. Without it, two concurrent
/// resolves of the *same* issue with *different* keeps could each read the
/// full present set before either deletes and, between them, delete every
/// copy — total loss for that issue. Holding this for the batch means the
/// second batch's per-group re-read (`resolve_one`) sees the first's deletes
/// and refuses the now-stale group. Resolve is not hot; a global lock is
/// cheap insurance on a permanent-delete path.
// ponytail: process-wide lock; if resolve ever needs throughput, key it per
// issue_id instead.
fn resolve_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Best-effort batch: each resolution is independent, so one refusal or one
/// undeletable file never sinks the rest. Every group re-runs the full safety
/// check server-side — the client's `kind`/suggestion is never trusted.
async fn resolve(
    State(state): State<AppState>,
    Json(body): Json<ResolveBody>,
) -> Result<Json<ResolveResponse>, ApiError> {
    // Serialize against concurrent resolves so a group's re-validation and
    // deletes are atomic w.r.t. another resolve touching the same issue.
    let _guard = resolve_lock().lock().await;
    let patterns = load_patterns(&state).await?;
    let roots: HashMap<i64, String> = library_root_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.path))
        .collect();

    let mut results = Vec::with_capacity(body.resolutions.len());
    for r in body.resolutions {
        let result = match resolve_one(&state, &patterns, &roots, r.issue_id, r.keep_file_id).await
        {
            Ok(res) => res,
            // A DB read failure mid-preflight becomes a per-group refusal
            // rather than failing the whole batch (matches reconcile's bulk
            // convention).
            Err(e) => ResolveResult {
                issue_id: r.issue_id,
                status: "refused",
                kept_file_id: None,
                deleted_file_ids: Vec::new(),
                failed: Vec::new(),
                reason: Some(e.to_string()),
            },
        };
        results.push(result);
    }
    Ok(Json(ResolveResponse { results }))
}

/// Re-validate one group from scratch and, if it passes, delete every loser.
/// Refuses (nothing touched) when: fewer than 2 present files, the keep isn't
/// among them, or the present files' issue numbers don't all agree (mismatch
/// — distinct issues, not a duplicate).
async fn resolve_one(
    state: &AppState,
    patterns: &[ParsingPattern],
    roots: &HashMap<i64, String>,
    issue_id: i64,
    keep_file_id: i64,
) -> Result<ResolveResult, ApiError> {
    let present: Vec<FileRow> = file_repo::list_by_issue(&state.db, issue_id)
        .await?
        .into_iter()
        .filter(|f| f.is_present)
        .collect();

    let refuse = |reason: &str| ResolveResult {
        issue_id,
        status: "refused",
        kept_file_id: None,
        deleted_file_ids: Vec::new(),
        failed: Vec::new(),
        reason: Some(reason.to_owned()),
    };

    if present.len() < 2 {
        return Ok(refuse(
            "issue has fewer than 2 present files — not a duplicate group",
        ));
    }
    if !present.iter().any(|f| f.id == keep_file_id) {
        return Ok(refuse(
            "keep_file_id is not one of this issue's present files",
        ));
    }
    let numbers: Vec<Option<String>> = present
        .iter()
        .map(|f| {
            parse_candidate_number(
                &f.path_relative,
                f.cached_comicinfo_xml.as_deref(),
                patterns,
            )
        })
        .collect();
    if !classify_is_duplicate(&numbers) {
        return Ok(refuse(
            "present files have mismatched issue numbers — distinct issues wrongly merged, not duplicates; refusing to delete",
        ));
    }

    let mut deleted_file_ids = Vec::new();
    let mut failed = Vec::new();
    for loser in present.iter().filter(|f| f.id != keep_file_id) {
        match delete_loser(state, roots, loser).await {
            Ok(()) => deleted_file_ids.push(loser.id),
            Err(err) => failed.push(ResolveFailure {
                file_id: loser.id,
                error: err,
            }),
        }
    }

    Ok(ResolveResult {
        issue_id,
        status: "resolved",
        kept_file_id: Some(keep_file_id),
        deleted_file_ids,
        failed,
        reason: None,
    })
}

/// Delete one loser's physical file then its DB row. Physical-first: a file
/// delete that succeeds followed by a failed row delete self-heals on the
/// next scan (is_present → 0), whereas the reverse would leave an untracked
/// orphan the scanner re-adds — recreating the very duplicate we're removing.
/// Already-missing file / already-gone row both count as success.
async fn delete_loser(
    state: &AppState,
    roots: &HashMap<i64, String>,
    loser: &FileRow,
) -> Result<(), String> {
    if !is_contained(&loser.path_relative) {
        tracing::warn!(
            file_id = loser.id,
            path = %loser.path_relative,
            "duplicate-files: refused non-contained path"
        );
        return Err("stored path is not contained within its library root".to_owned());
    }
    let root = roots
        .get(&loser.library_root_id)
        .ok_or_else(|| format!("unknown library_root_id {}", loser.library_root_id))?;
    let abs = Path::new(root).join(&loser.path_relative);

    // `is_contained` above is purely lexical (rejects `..`/absolute). Delete
    // is higher-stakes than the read paths that share that guard, so also
    // resolve symlinks and confirm the real target is still inside the real
    // library root — a symlinked parent directory can't redirect the delete
    // outside. Canonicalize fails with NotFound iff the file is already gone,
    // which we treat as success.
    let canon_root = tokio::fs::canonicalize(root)
        .await
        .map_err(|e| format!("cannot resolve library root path: {e}"))?;
    match tokio::fs::canonicalize(&abs).await {
        Ok(canon_target) => {
            if !canon_target.starts_with(&canon_root) {
                tracing::warn!(
                    file_id = loser.id,
                    path = %loser.path_relative,
                    resolved = %canon_target.display(),
                    "duplicate-files: refused path resolving outside library root"
                );
                return Err("resolved path escapes its library root".to_owned());
            }
            if let Err(e) = tokio::fs::remove_file(&canon_target).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(format!("failed to delete file: {e}"));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Already gone on disk — treat as success and clean up the row.
            tracing::info!(file_id = loser.id, path = %abs.display(), "duplicate-files: file already absent");
        }
        Err(e) => return Err(format!("cannot resolve file path: {e}")),
    }

    match file_repo::delete(&state.db, loser.id).await {
        Ok(()) | Err(longbox_db::DbError::NotFound) => Ok(()),
        Err(e) => Err(format!(
            "file deleted from disk but DB row removal failed: {e}"
        )),
    }
}

// -------- pure helpers --------

/// Lowercased archive extension bucket.
fn derive_format(path_relative: &str) -> &'static str {
    match path_relative
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "cbz" => "cbz",
        "cbr" => "cbr",
        "cb7" => "cb7",
        _ => "other",
    }
}

/// True when any path segment is `_unsorted` (case-insensitive) — the
/// deprecated staging area, a less-canonical home than the real library.
fn under_unsorted(path_relative: &str) -> bool {
    path_relative
        .split('/')
        .any(|seg| seg.eq_ignore_ascii_case("_unsorted"))
}

/// A candidate is suspect when it's below the absolute floor OR drastically
/// smaller than its largest sibling — a likely truncated/corrupt copy.
fn is_suspect_size(size: i64, largest_sibling: i64) -> bool {
    size < SUSPECT_ABS_FLOOR_BYTES || size.saturating_mul(SUSPECT_RATIO_DENOM) < largest_sibling
}

/// Parse this file's own issue number: filename via the enabled patterns
/// first, cached ComicInfo `<Number>` as fallback. `None` when neither yields
/// one — which forces the group to `mismatch` (we won't delete on a number we
/// can't confirm).
fn parse_candidate_number(
    path_relative: &str,
    cached_xml: Option<&str>,
    patterns: &[ParsingPattern],
) -> Option<String> {
    let basename = path_relative.rsplit('/').next().unwrap_or(path_relative);
    if let Some(p) = parse_filename(basename, patterns) {
        return Some(p.number);
    }
    let xml = cached_xml?;
    ComicInfo::parse(xml.as_bytes()).ok()?.number
}

/// Normalise an issue number for equality: strip leading zeros from a
/// pure-digit string (`001` == `1`), else lowercase+trim. Deliberately
/// conservative — anything it can't confidently equate stays distinct, which
/// pushes an uncertain group to `mismatch` (safe: no deletion) rather than
/// risking a wrong-file delete.
fn norm_num(n: &str) -> String {
    let t = n.trim();
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        let s = t.trim_start_matches('0');
        return if s.is_empty() {
            "0".to_owned()
        } else {
            s.to_owned()
        };
    }
    t.to_ascii_lowercase()
}

/// A group is a true duplicate only when every candidate parsed a number and
/// they all agree. Any unparsed number, or any disagreement, → not a
/// duplicate (mismatch).
fn classify_is_duplicate(numbers: &[Option<String>]) -> bool {
    let mut it = numbers.iter();
    let first = match it.next() {
        Some(Some(n)) => norm_num(n),
        _ => return false,
    };
    numbers
        .iter()
        .all(|n| matches!(n, Some(x) if norm_num(x) == first))
}

/// The default keep suggestion. Never a suspect-size candidate unless every
/// candidate is suspect (then the largest, as least-bad). Among the eligible
/// pool the precedence is: NOT under `_unsorted/` first, then `.cbz` over
/// other formats, then larger size, then lower file id (stable).
fn suggest_keep(files: &[DupCandidate]) -> Option<i64> {
    if files.is_empty() {
        return None;
    }
    let healthy: Vec<&DupCandidate> = files.iter().filter(|f| !f.suspect_corrupt).collect();
    let pool: Vec<&DupCandidate> = if healthy.is_empty() {
        files.iter().collect()
    } else {
        healthy
    };
    pool.into_iter()
        .max_by_key(|f| {
            (
                !f.under_unsorted,            // true (not staging) preferred
                f.format == "cbz",            // cbz preferred
                f.size_bytes,                 // larger preferred
                std::cmp::Reverse(f.file_id), // lower id as tiebreak
            )
        })
        .map(|f| f.file_id)
}

/// Load the enabled parsing patterns into `longbox-core`'s shape — same
/// mapping the scanner and reconcile use.
async fn load_patterns(state: &AppState) -> Result<Vec<ParsingPattern>, ApiError> {
    Ok(parsing_pattern_repo::list_enabled(&state.db)
        .await?
        .into_iter()
        .map(|r| ParsingPattern {
            id: r.id,
            name: r.name,
            pattern: r.pattern,
            priority: i32::try_from(r.priority).unwrap_or(i32::MAX),
            enabled: r.enabled,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(file_id: i64, path: &str, size: i64, suspect: bool, unsorted: bool) -> DupCandidate {
        DupCandidate {
            file_id,
            path_relative: path.to_owned(),
            size_bytes: size,
            format: derive_format(path).to_owned(),
            parsed_number: None,
            is_served: false,
            suspect_corrupt: suspect,
            under_unsorted: unsorted,
        }
    }

    #[test]
    fn format_derivation() {
        assert_eq!(derive_format("a/b.cbz"), "cbz");
        assert_eq!(derive_format("a/b.CBR"), "cbr");
        assert_eq!(derive_format("a/b.cb7"), "cb7");
        assert_eq!(derive_format("a/b.pdf"), "other");
        assert_eq!(derive_format("noext"), "other");
    }

    #[test]
    fn unsorted_detection() {
        assert!(under_unsorted("_unsorted/Saga 1.cbz"));
        assert!(under_unsorted("a/_Unsorted/x.cbz"));
        assert!(!under_unsorted("Saga (2012)/Saga 1.cbz"));
    }

    #[test]
    fn suspect_size_floor_and_ratio() {
        // Under the 1 MiB absolute floor.
        assert!(is_suspect_size(607, 100_000_000));
        // 0.6 MB next to 92 MB → under 1/10 ratio.
        assert!(is_suspect_size(600_000, 92_000_000));
        // Healthy large file.
        assert!(!is_suspect_size(101_500_000, 101_500_000));
        // A modest-but-fine file next to a same-size sibling.
        assert!(!is_suspect_size(5_000_000, 6_000_000));
    }

    #[test]
    fn classify_true_duplicate_when_numbers_agree() {
        let nums = vec![Some("001".to_owned()), Some("1".to_owned())];
        assert!(classify_is_duplicate(&nums)); // 001 == 1
    }

    #[test]
    fn classify_mismatch_when_numbers_disagree() {
        // The real "Ferocious" case: 001, 002, 005 under one issue_id.
        let nums = vec![
            Some("1".to_owned()),
            Some("2".to_owned()),
            Some("5".to_owned()),
        ];
        assert!(!classify_is_duplicate(&nums));
    }

    #[test]
    fn classify_mismatch_when_any_unparsed() {
        let nums = vec![Some("1".to_owned()), None];
        assert!(!classify_is_duplicate(&nums));
    }

    #[test]
    fn suggest_keep_prefers_library_over_unsorted_then_cbz_then_size() {
        // A .cbz in _unsorted vs a .cbr in the real library → library wins
        // (location precedes format).
        let files = vec![
            cand(1, "_unsorted/Saga 1.cbz", 90_000_000, false, true),
            cand(2, "Saga (2012)/Saga 1.cbr", 90_000_000, false, false),
        ];
        assert_eq!(suggest_keep(&files), Some(2));

        // Same location, cbz beats cbr.
        let files = vec![
            cand(3, "Saga/Saga 1.cbr", 90_000_000, false, false),
            cand(4, "Saga/Saga 1.cbz", 90_000_000, false, false),
        ];
        assert_eq!(suggest_keep(&files), Some(4));

        // Same location + format, larger wins.
        let files = vec![
            cand(5, "Saga/a.cbz", 10_000_000, false, false),
            cand(6, "Saga/b.cbz", 90_000_000, false, false),
        ];
        assert_eq!(suggest_keep(&files), Some(6));
    }

    #[test]
    fn suggest_keep_never_suggests_a_suspect_corrupt_copy() {
        // The real "Darkness" case: a 607-byte cbz next to a healthy cbr.
        // Even though cbz would normally win on format, the tiny file is
        // suspect and must not be suggested.
        let files = vec![
            cand(1, "Darkness/001.cbz", 607, true, false),
            cand(2, "Darkness/001.cbr", 101_500_000, false, false),
        ];
        assert_eq!(suggest_keep(&files), Some(2));
    }

    #[test]
    fn suggest_keep_falls_back_to_largest_when_all_suspect() {
        let files = vec![
            cand(1, "x/a.cbz", 500, true, false),
            cand(2, "x/b.cbz", 900, true, false),
        ];
        assert_eq!(suggest_keep(&files), Some(2));
    }
}
