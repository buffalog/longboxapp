//! Library Tidy — duplicate physical files.
//!
//! An issue with more than one `is_present = 1` file has multiple physical
//! copies of the same comic (split across folders, or same folder under
//! different names/formats). This surfaces them and lets a human keep one
//! copy and permanently delete the rest — file on disk AND catalog row.
//!
//! Three safety rails, because a bug here deletes real user files:
//!
//! 0. **Cross-folder exclusion.** A group whose files live under different
//!    series folders is never a duplicate group, whatever the parsed numbers
//!    say — and in the failure mode that matters the numbers agree perfectly.
//!    Two volumes of one title (`The Authority (1999)` and `(2008)`) have a #4
//!    each; when the matcher binds both onto the 1999 series' issue #4 they
//!    present as a textbook duplicate pair. 26 of the 28 groups this endpoint
//!    surfaced on the live library were that shape, and resolving them would
//!    have permanently deleted 26 distinct comics.
//!
//!    Folder comparison here is byte-exact, never normalized: deciding which
//!    name differences are safe to ignore is a judgement call, and a
//!    permanent-delete path is the wrong place for one. Normalization is used
//!    only to word the finding (`cross_folder_same_series` vs
//!    `cross_folder_wrong_series`), never to permit a delete.
//!
//! 1. **Mismatch exclusion.** Some issues have >1 present file not because of
//!    duplication but because distinct issues were wrongly matched to one
//!    issue row (e.g. files for #1, #2, #5 all under one `issue_id`). Those
//!    are NOT duplicates — deleting the "losers" would destroy distinct
//!    issues. Every candidate's own issue number is parsed from its filename
//!    (never from its ComicInfo — see `parse_filename_number`); a group is
//!    actionable only when they all agree.
//!    Mismatch groups get `kind = "mismatch"` and the delete resolver
//!    independently re-validates and refuses them.
//!
//!    A mismatch group is repaired by *re-pointing* the stray files, not by
//!    deleting them — see the `correct` endpoint. Root cause: some files carry
//!    a wrong `<Number>` in their embedded ComicInfo.xml, and Tier 2 of the
//!    match cascade believes it over the (correct) filename. Nothing physical
//!    happens on a correction; it's a plain `files.issue_id` FK fix to an
//!    issue row that already exists.
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

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::Path;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_core::{parse_filename_with_normalization, FileStatus, ParsingPattern};
use longbox_db::{
    file_repo, issue_repo, library_root_repo, parsing_pattern_repo, DuplicateFileCandidate, FileRow,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::pathsafe::is_contained;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/library/tidy/duplicate-files", get(list))
        .route("/library/tidy/duplicate-files/resolve", post(resolve))
        .route("/library/tidy/duplicate-files/correct", post(correct))
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
    /// The issue number parsed from this file's name. `None` when it doesn't
    /// parse — which keeps the group out of the deletable `duplicate` bucket.
    parsed_number: Option<String>,
    /// For a file in a `mismatch` group: the issue row in the SAME series
    /// whose number matches this file's *filename*-parsed number — i.e. where
    /// this file should have been pointed all along. `None` when the
    /// suggestion isn't unambiguous (see [`propose_targets`] /
    /// [`target_accepts`]) — those files stay manual-review-only.
    suggested_issue_id: Option<i64>,
    /// The copy the app currently serves (owned-preferred, then lowest id).
    is_served: bool,
    /// Size looks corrupt relative to the floor / its siblings — flagged so a
    /// reviewer's eye catches it, and never auto-suggested as the keep.
    suspect_corrupt: bool,
    /// Lives under a `_unsorted/` staging path (less canonical).
    under_unsorted: bool,
    /// The issue number this file's own filename names, when this series has
    /// NO catalog record for it — e.g. a file parsing to #5 in a series whose
    /// catalog stops at #4.
    ///
    /// A distinct, first-class state, NOT a confidence problem. The old UI
    /// reported it as "no confident suggestion — your call" while instructing
    /// the user to "move each stray file to the issue its filename says it
    /// is", which is an instruction to do something impossible. The catalog
    /// is simply missing the issue; the fix is to refresh the series from
    /// ComicVine, not to make a judgement call.
    missing_target_number: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
struct DupGroup {
    issue_id: i64,
    series_id: i64,
    series_title: String,
    issue_number: String,
    /// `duplicate` (all candidates agree on issue number → actionable) or
    /// `mismatch` (numbers disagree → distinct issues wrongly merged; NOT
    /// deletable here — re-point the strays instead, see `correct`).
    kind: &'static str,
    /// Pre-selected keep — a hint only; `Some` only for `duplicate` groups.
    suggested_keep_file_id: Option<i64>,
    /// Every issue row in this group's series, so the UI can offer an
    /// override when it disagrees with (or we can't produce) a suggestion.
    /// Only populated for `mismatch` groups.
    issue_options: Vec<IssueOption>,
    files: Vec<DupCandidate>,
}

#[derive(Debug, Serialize, PartialEq)]
struct IssueOption {
    issue_id: i64,
    number: String,
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
    let roots: HashMap<i64, String> = library_root_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.path))
        .collect();
    let digests = validated_digests(&state, &roots, &candidates).await;

    let mut groups = build_groups(candidates, &patterns, &digests);
    annotate_suggestions(&state, &patterns, &mut groups).await?;
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
    digests: Digests<'_>,
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
        groups.push(build_one_group(
            &candidates[i..j],
            patterns,
            &digests[i..j],
        ));
        i = j;
    }
    groups
}

/// Validate each candidate's stored digest against the file on disk.
///
/// The stored digest is only meaningful for the bytes it was computed from,
/// and the catalog's `size_bytes`/`mtime` are themselves only as fresh as the
/// last scan — which runs once a day. So this stats each file and compares the
/// digest's own stamp against what disk reports right now. A file that has
/// changed since it was hashed yields `None` and its group falls to
/// `pending_analysis` rather than being classified from a digest describing
/// bytes that are gone.
///
/// Stats only, never hashes: this is a request path. Populating digests is the
/// background job's job.
async fn validated_digests(
    state: &AppState,
    roots: &HashMap<i64, String>,
    candidates: &[DuplicateFileCandidate],
) -> Vec<Option<String>> {
    let mut out = Vec::with_capacity(candidates.len());
    for c in candidates {
        out.push(validate_one_digest(state, roots, c).await);
    }
    out
}

async fn validate_one_digest(
    _state: &AppState,
    roots: &HashMap<i64, String>,
    c: &DuplicateFileCandidate,
) -> Option<String> {
    validate_digest(
        roots,
        c.library_root_id,
        &c.path_relative,
        c.content_blake3.as_deref(),
        c.hashed_size_bytes,
        c.hashed_mtime,
    )
    .await
    .1
}

/// Stat the file and decide whether its stored digest still describes it.
/// Returns (file exists on disk, trustworthy digest).
async fn validate_digest(
    roots: &HashMap<i64, String>,
    library_root_id: i64,
    path_relative: &str,
    digest: Option<&str>,
    stamp_size: Option<i64>,
    stamp_mtime: Option<time::PrimitiveDateTime>,
) -> (bool, Option<String>) {
    if !is_contained(path_relative) {
        return (false, None);
    }
    let Some(root) = roots.get(&library_root_id) else {
        return (false, None);
    };
    let Ok(meta) = tokio::fs::metadata(Path::new(root).join(path_relative)).await else {
        return (false, None);
    };
    let Some(digest) = digest.filter(|d| !d.is_empty()) else {
        return (true, None);
    };
    let (Some(stamp_size), Some(stamp_mtime)) = (stamp_size, stamp_mtime) else {
        return (true, None);
    };
    let observed_size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    let Ok(modified) = meta.modified() else {
        return (true, None);
    };
    let off = time::OffsetDateTime::from(modified).to_offset(time::UtcOffset::UTC);
    let observed_mtime = time::PrimitiveDateTime::new(off.date(), off.time());
    let fresh = stamp_size == observed_size && stamp_mtime == observed_mtime;
    (true, fresh.then(|| digest.to_owned()))
}

/// The content verdict for a resolve, computed over the files that actually
/// exist on disk.
struct ContentJudgement {
    verdict: ContentVerdict,
    on_disk_count: usize,
    on_disk_ids: Vec<i64>,
}

async fn judge_content(
    roots: &HashMap<i64, String>,
    present: &[FileRow],
    stamps: &HashMap<i64, longbox_db::ContentStamp>,
) -> ContentJudgement {
    let mut on_disk_ids = Vec::new();
    let mut digests = Vec::new();
    for f in present {
        let stamp = stamps.get(&f.id);
        let (exists, digest) = validate_digest(
            roots,
            f.library_root_id,
            &f.path_relative,
            stamp.and_then(|s| s.content_blake3.as_deref()),
            stamp.and_then(|s| s.hashed_size_bytes),
            stamp.and_then(|s| s.hashed_mtime),
        )
        .await;
        if exists {
            on_disk_ids.push(f.id);
            digests.push(digest);
        }
    }
    ContentJudgement {
        verdict: classify_content(&digests),
        on_disk_count: on_disk_ids.len(),
        on_disk_ids,
    }
}

/// Per-file content digest, already validated against disk by the caller.
/// `None` means "no trustworthy digest for these bytes" — either never hashed
/// or hashed against a version that has since changed.
type Digests<'a> = &'a [Option<String>];

fn build_one_group(
    rows: &[DuplicateFileCandidate],
    patterns: &[ParsingPattern],
    digests: Digests<'_>,
) -> DupGroup {
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
            parsed_number: parse_filename_number(&r.path_relative, patterns),
            // Filled in by `annotate_suggestions` (needs the DB).
            suggested_issue_id: None,
            is_served: Some(r.file_id) == served_id,
            suspect_corrupt: is_suspect_size(r.size_bytes, largest),
            under_unsorted: under_unsorted(&r.path_relative),
            // Filled in by `annotate_suggestions` (needs the series catalog).
            missing_target_number: None,
        })
        .collect();

    // Cross-folder check runs FIRST and outranks everything. A group whose
    // files live under different series folders is not a duplicate group no
    // matter how well the parsed numbers agree — and agreement is exactly what
    // the failure mode produces. Live case: `The Authority (1999)/…004.cbz`
    // and `The Authority (2008)/…004.cbr` both parse to 4, both bound to the
    // 1999 series' issue #4, and both are real, different comics. 26 of the 28
    // groups Tidy showed were this shape; clicking through would have deleted
    // 26 distinct issues.
    let folders: Vec<&str> = files
        .iter()
        .map(|f| top_level_folder(&f.path_relative))
        .collect();
    let cross_folder = spans_multiple_folders(&folders);

    // Content identity replaces filename-number agreement as the thing that
    // decides deletability. Filenames describe what someone *called* a file;
    // only the bytes say whether two files are the same comic. Both live
    // failure modes came from believing the names:
    //   - Hello Darkness 001/002 are byte-identical and were classified
    //     `mismatch`, so Tidy offered to "move" them to issues #1 and #2 —
    //     marking #2 owned while it holds #1's content.
    //   - Death Fight Forever 003/005 are byte-identical; 005 is a misnamed
    //     copy. The correct action is delete, not move.
    let content = classify_content(digests);

    let is_dup = !cross_folder && content == ContentVerdict::Identical;
    let suggested_keep_file_id = if is_dup { suggest_keep(&files) } else { None };

    let kind = if cross_folder {
        if folders_agree_on_series(&folders) {
            "cross_folder_same_series"
        } else {
            "cross_folder_wrong_series"
        }
    } else {
        match content {
            // No trustworthy digest for at least one file. Absence of a hash
            // is not evidence of sameness OR difference, so this must not
            // fall through to a verdict that renders a delete button — the
            // same reasoning as the stale-digest guard in `content_hash`.
            ContentVerdict::Unknown => "pending_analysis",
            ContentVerdict::Identical => "duplicate",
            ContentVerdict::Distinct => "mismatch",
        }
    };

    DupGroup {
        issue_id: rows[0].issue_id,
        series_id: rows[0].series_id,
        series_title: rows[0].series_title.clone(),
        issue_number: rows[0].issue_number.clone(),
        kind,
        suggested_keep_file_id,
        issue_options: Vec::new(),
        files,
    }
}

// -------- content identity --------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentVerdict {
    /// Every file has a trustworthy digest and they all agree.
    Identical,
    /// Every file has a trustworthy digest and at least two differ.
    Distinct,
    /// At least one file has no digest we can trust right now.
    Unknown,
}

/// Decide content identity from per-file digests that the caller has already
/// validated against disk.
///
/// `Unknown` is returned whenever ANY file is missing a trustworthy digest,
/// not just when all are. A group of three where two match and the third is
/// unhashed is not "two duplicates plus an unknown" — it is a group we cannot
/// classify, because the unhashed file might be identical to the others (so
/// deleting a "duplicate" loses nothing) or might be a distinct issue (so
/// deleting it loses a comic). Partial evidence is not partial permission.
fn classify_content(digests: Digests<'_>) -> ContentVerdict {
    if digests.len() < 2 || digests.iter().any(|d| d.is_none()) {
        return ContentVerdict::Unknown;
    }
    if digests.windows(2).all(|w| w[0] == w[1]) {
        ContentVerdict::Identical
    } else {
        ContentVerdict::Distinct
    }
}

// -------- cross-folder detection --------

/// The first path component — the series folder a file lives under.
/// Root-level files (no separator) have no series folder and yield `""`,
/// which compares unequal to any real folder and so is treated as its own
/// location. That is the safe direction: it can only suppress a delete.
fn top_level_folder(path_relative: &str) -> &str {
    path_relative
        .split('/')
        .next()
        .filter(|first| *first != path_relative)
        .unwrap_or("")
}

/// Strict, byte-exact comparison of raw folder names. Deliberately not
/// normalized: this decides whether the delete button exists at all, and any
/// normalization is a rule about which differences are safe to ignore — a
/// judgement that has no place on a permanent-delete path. Two folders whose
/// names differ in any way are two locations.
fn spans_multiple_folders(folders: &[&str]) -> bool {
    folders.windows(2).any(|w| w[0] != w[1])
}

/// Whether differing folder names still describe the same series — used ONLY
/// to choose wording, never to permit a delete.
///
/// Same series iff the titles normalize equal AND their years are compatible
/// (equal, or at least one folder carries no year). The year test is what
/// separates the benign case from the dangerous one, and dropping it would
/// invert them: `Drifter` vs `Drifter (2014)` is one series stored under two
/// folder names, while `The Authority (1999)` vs `The Authority (2008)` is two
/// different comics whose titles normalize identically.
///
/// Errs toward "wrong series" when unsure. Over-warning on a group whose
/// delete button is already suppressed costs a second look; under-warning
/// tells someone their two distinct volumes are the same book.
fn folders_agree_on_series(folders: &[&str]) -> bool {
    let first = folder_identity(folders[0]);
    folders.iter().skip(1).all(|f| {
        let other = folder_identity(f);
        first.0 == other.0 && (first.1 == other.1 || first.1.is_none() || other.1.is_none())
    })
}

/// Split a series folder name into (normalized title, volume year).
///
/// The year must come off before normalizing: `normalize_title` drops the
/// parentheses but keeps the digits as a token, so `Drifter (2014)` would
/// normalize to `drifter 2014` and never match `drifter`.
fn folder_identity(folder: &str) -> (String, Option<i32>) {
    let trimmed = folder.trim_end();
    let (title, year) = match trimmed.rfind('(') {
        Some(open) if trimmed.ends_with(')') => {
            let inner = &trimmed[open + 1..trimmed.len() - 1];
            match inner.parse::<i32>() {
                Ok(y) => (&trimmed[..open], Some(y)),
                // Parenthesised but not a year (`Ultramega (Deluxe)`) — leave
                // it in the title, it is part of how the folder is named.
                Err(_) => (trimmed, None),
            }
        }
        _ => (trimmed, None),
    };
    (longbox_core::normalize_title(title), year)
}

// -------- suggested corrections (mismatch groups) --------

/// For every `mismatch` group, fill in each stray file's `suggested_issue_id`
/// and the group's `issue_options` (override list).
///
/// A mismatch group is N distinct issues wrongly collapsed onto one issue row.
/// The fix is a pointer correction, not a delete: look up the issue row in the
/// same series whose number equals the file's *filename*-parsed number, and
/// suggest it. Every suggestion must be unambiguous, so we bail on:
///
/// - the filename yielding no number,
/// - two files in the group claiming the same number (which of them is really
///   that issue? we can't know),
/// - two issue rows in the series sharing a normalized number (duplicate
///   catalog rows — pick one and we may pick wrong),
/// - the target row not existing,
/// - the target already holding a present file that parses to a *different*
///   number (re-pointing there would just create a fresh mismatch).
///
/// A target already holding a present file with the *same* number is fine: the
/// re-point turns it into an honest duplicate group, which the delete resolver
/// then handles as usual.
// ponytail: one query per distinct series + one per distinct proposed target,
// both cached per request. At the real ~28 mismatch groups that's trivial; the
// ceiling is O(groups × files) round-trips at per_page=200. Collapse to two
// `IN` queries if a library ever gets bad enough for that to bite.
async fn annotate_suggestions(
    state: &AppState,
    patterns: &[ParsingPattern],
    groups: &mut [DupGroup],
) -> Result<(), ApiError> {
    // Cache across groups: the same series can own several mismatch groups.
    let mut series_issues: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
    let mut present_numbers: HashMap<i64, Vec<Option<String>>> = HashMap::new();

    for group in groups.iter_mut().filter(|g| g.kind == "mismatch") {
        if let Entry::Vacant(slot) = series_issues.entry(group.series_id) {
            let issues = issue_repo::list_by_series(&state.db, group.series_id).await?;
            slot.insert(issues.into_iter().map(|i| (i.number, i.id)).collect());
        }
        let issues = &series_issues[&group.series_id];

        let filename_numbers: Vec<Option<String>> = group
            .files
            .iter()
            .map(|f| parse_filename_number(&f.path_relative, patterns))
            .collect();

        // Flag every stray whose filename names an issue this series simply
        // does not have. That is an absence in the catalog, not a failure of
        // nerve, and it gets said plainly instead of being folded into the
        // "no confident suggestion" bucket with genuinely ambiguous files.
        for (idx, number) in filename_numbers.iter().enumerate() {
            let Some(number) = number.as_deref() else { continue };
            let known = issues.iter().any(|(n, _)| norm_num(n) == norm_num(number));
            if !known {
                group.files[idx].missing_target_number = Some(norm_num(number));
            }
        }

        for (idx, target) in propose_targets(group.issue_id, &filename_numbers, issues)
            .into_iter()
            .enumerate()
        {
            let Some(target) = target else { continue };
            if let Entry::Vacant(slot) = present_numbers.entry(target) {
                let nums = file_repo::list_by_issue(&state.db, target)
                    .await?
                    .iter()
                    .filter(|f| f.is_present)
                    .map(|f| parse_filename_number(&f.path_relative, patterns))
                    .collect();
                slot.insert(nums);
            }
            // A `Some(target)` implies a `Some` number — but don't lean on
            // that: an empty claim would vacuously satisfy `target_accepts`.
            let Some(claimed) = filename_numbers[idx].as_deref() else {
                continue;
            };
            if target_accepts(claimed, &present_numbers[&target]) {
                group.files[idx].suggested_issue_id = Some(target);
            }
        }

        group.issue_options = issues
            .iter()
            .map(|(number, issue_id)| IssueOption {
                issue_id: *issue_id,
                number: number.clone(),
            })
            .collect();
    }
    Ok(())
}

/// Per-file target issue (aligned with `filename_numbers`), before the
/// occupancy check. `None` for any file we can't confidently place — see
/// [`annotate_suggestions`] for the rules. Pure, so the ambiguity rules are
/// testable without a DB.
fn propose_targets(
    current_issue_id: i64,
    filename_numbers: &[Option<String>],
    series_issues: &[(String, i64)],
) -> Vec<Option<i64>> {
    let count_of = |target: &str| -> usize {
        filename_numbers
            .iter()
            .filter(|n| n.as_deref().map(norm_num).as_deref() == Some(target))
            .count()
    };

    filename_numbers
        .iter()
        .map(|number| {
            let number = norm_num(number.as_deref()?);
            // Two files claiming the same number: can't tell them apart.
            if count_of(&number) > 1 {
                return None;
            }
            let mut rows = series_issues
                .iter()
                .filter(|(n, _)| norm_num(n) == number)
                .map(|(_, id)| *id);
            let target = rows.next()?;
            // Duplicate catalog rows for one number — refuse to guess.
            if rows.next().is_some() {
                return None;
            }
            // Already where it belongs (this is the file the group is
            // *named* after) — nothing to correct.
            if target == current_issue_id {
                return None;
            }
            Some(target)
        })
        .collect()
}

/// May a file claiming issue number `number` be re-pointed at a target issue
/// whose present files parse to `existing`? Only when every file already
/// sitting there agrees on the number (or there are none) — an occupant with a
/// different number means the target is *itself* mismatched, and moving a file
/// in would tangle it further.
fn target_accepts(number: &str, existing: &[Option<String>]) -> bool {
    let number = norm_num(number);
    existing
        .iter()
        .all(|n| matches!(n, Some(x) if norm_num(x) == number))
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
    // No parsing patterns here any more: the delete decision is made from
    // content digests, not filenames. Loading them would only invite a
    // filename-based fallback back onto the permanent-delete path.
    let roots: HashMap<i64, String> = library_root_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|r| (r.id, r.path))
        .collect();

    let mut results = Vec::with_capacity(body.resolutions.len());
    for r in body.resolutions {
        let result = match resolve_one(&state, &roots, r.issue_id, r.keep_file_id).await
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
    // Cross-folder refusal, re-derived here rather than trusted from the
    // client's `kind`. This check must live on the write path, not just in the
    // GET payload: the list response is a hint, and a delete that is only
    // prevented by the UI is not prevented at all.
    //
    // It also has to come BEFORE the number check, because the dangerous case
    // passes the number check — files from two volumes of one title parse to
    // the same issue number and look like textbook duplicates.
    let folders: Vec<&str> = present
        .iter()
        .map(|f| top_level_folder(&f.path_relative))
        .collect();
    if spans_multiple_folders(&folders) {
        return Ok(refuse(
            "present files span more than one series folder — a wrong-series match, not a duplicate; refusing to delete",
        ));
    }

    // Content identity, re-derived from disk, is now the authority on whether
    // a delete is safe. It REPLACES the filename-number check rather than
    // supplementing it: byte-identical files with disagreeing numbers are
    // precisely the case this endpoint must now allow (Hello Darkness 001 and
    // 002 are the same bytes; Death Fight Forever 005 is a misnamed copy of
    // 003). Keeping the old check would refuse exactly what content identity
    // exists to permit.
    //
    // Only files that actually exist on disk take part in the verdict. A row
    // whose file is already gone cannot be a content participant, and
    // deleting it cannot lose anything — that stays a safe cleanup.
    let stamps: HashMap<i64, longbox_db::ContentStamp> =
        file_repo::content_stamps_by_issue(&state.db, issue_id)
            .await?
            .into_iter()
            .map(|s| (s.file_id, s))
            .collect();
    let judged = judge_content(roots, &present, &stamps).await;

    if judged.on_disk_count >= 2 {
        match judged.verdict {
            ContentVerdict::Identical => {}
            ContentVerdict::Distinct => {
                return Ok(refuse(
                    "present files have different content — distinct issues wrongly merged, not duplicates; refusing to delete",
                ));
            }
            // Refusing on Unknown is the whole point. An unhashed or
            // stale-hashed group has produced no evidence, and no evidence
            // must never read as permission to delete.
            ContentVerdict::Unknown => {
                return Ok(refuse(
                    "present files have not been content-analyzed yet — refusing to delete without knowing whether they are the same comic",
                ));
            }
        }
    }

    // Never keep a ghost while deleting real files: if anything is on disk,
    // the survivor has to be one of those.
    if judged.on_disk_count > 0 && !judged.on_disk_ids.contains(&keep_file_id) {
        return Ok(refuse(
            "keep_file_id names a file that is missing from disk while other copies exist; refusing to delete the real ones",
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

// -------- POST: correct (re-point a mismatched file) --------

#[derive(Debug, Deserialize)]
struct CorrectBody {
    file_id: i64,
    /// The issue this file should have been pointed at all along.
    issue_id: i64,
}

#[derive(Debug, Serialize)]
struct CorrectResult {
    file_id: i64,
    from_issue_id: i64,
    to_issue_id: i64,
}

/// Re-point one file's `issue_id`. Nothing physical happens — the file stays
/// on disk, the target issue row already exists; this is a plain FK fix.
///
/// One correction per request, explicitly confirmed by a human. Everything is
/// re-derived server-side and the client's target is never taken on faith: the
/// file's own filename has to independently agree that the target is where it
/// belongs. Any doubt → 422, nothing written.
///
/// The corrected row is stamped `manual` / confidence 1.0. That's what makes
/// the fix stick: the file's embedded ComicInfo `<Number>` is still wrong and
/// still there, so without the manual marker the next scan's Tier 2 would
/// happily re-point it back at the wrong issue.
async fn correct(
    State(state): State<AppState>,
    Json(body): Json<CorrectBody>,
) -> Result<Json<CorrectResult>, ApiError> {
    // Share the resolver's lock: a concurrent delete-resolve reads the same
    // present-file sets we validate against.
    let _guard = resolve_lock().lock().await;
    let patterns = load_patterns(&state).await?;

    let refuse = |message: String| ApiError::Unprocessable {
        code: "unprocessable.correction_refused",
        message,
    };

    let file = file_repo::find_by_id(&state.db, body.file_id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "file",
            id: body.file_id.to_string(),
        })?;
    if !file.is_present {
        return Err(refuse("file is not present on disk".to_owned()));
    }
    // The user has explicitly disclaimed this file. The scanner is careful to
    // preserve that; so are we. (Our write would stamp it `owned` and quietly
    // un-ignore it.)
    if file.status == FileStatus::Ignored.as_db_str() {
        return Err(refuse(
            "file is marked ignored — un-ignore it first if you want it filed".to_owned(),
        ));
    }
    let Some(from_issue_id) = file.issue_id else {
        return Err(refuse(
            "file is not matched to any issue — nothing to correct".to_owned(),
        ));
    };
    if from_issue_id == body.issue_id {
        return Err(refuse("file already points at that issue".to_owned()));
    }
    // A row that's already `manual` IS re-pointable: this request is itself a
    // human's explicit click, so a previous human decision can be superseded
    // by a newer one. Refusing here would make a mis-correction unfixable
    // through the UI — and the filename gate below still has to corroborate
    // the new target either way.

    let from_issue = issue_repo::find_by_id(&state.db, from_issue_id)
        .await?
        .ok_or_else(|| refuse("the file's current issue no longer exists".to_owned()))?;
    let to_issue = issue_repo::find_by_id(&state.db, body.issue_id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "issue",
            id: body.issue_id.to_string(),
        })?;
    if to_issue.series_id != from_issue.series_id {
        return Err(refuse(
            "target issue belongs to a different series — this endpoint only re-points within one series".to_owned(),
        ));
    }

    // CONTENT IDENTITY DOMINATES TARGET AVAILABILITY.
    //
    // A file that is byte-identical to another file on its current issue is a
    // duplicate, not a stray, and the correct action is to delete it — never
    // to re-point it. The hazard is specifically a refresh: pulling new issue
    // records from ComicVine CREATES move targets, and without this a
    // freshly-created issue would become a legal destination for a copy of a
    // book we already hold. Taking that move marks the new issue owned while
    // it holds another issue's content — the exact phantom-ownership pattern
    // this whole feature exists to eliminate, manufactured by the repair
    // action for it.
    //
    // Checked here rather than only in the list payload because the list is a
    // hint. `annotate_suggestions` already withholds move UI from a
    // content-identical group, but a hand-rolled POST bypasses the UI
    // entirely, and this endpoint is the thing that actually writes.
    let siblings: Vec<FileRow> = file_repo::list_by_issue(&state.db, from_issue_id)
        .await?
        .into_iter()
        .filter(|f| f.is_present)
        .collect();
    if siblings.len() > 1 {
        let roots: HashMap<i64, String> = library_root_repo::list_all(&state.db)
            .await?
            .into_iter()
            .map(|r| (r.id, r.path))
            .collect();
        let stamps: HashMap<i64, longbox_db::ContentStamp> =
            file_repo::content_stamps_by_issue(&state.db, from_issue_id)
                .await?
                .into_iter()
                .map(|s| (s.file_id, s))
                .collect();
        let mut mine = None;
        let mut others = Vec::new();
        for f in &siblings {
            let stamp = stamps.get(&f.id);
            let (_, digest) = validate_digest(
                &roots,
                f.library_root_id,
                &f.path_relative,
                stamp.and_then(|s| s.content_blake3.as_deref()),
                stamp.and_then(|s| s.hashed_size_bytes),
                stamp.and_then(|s| s.hashed_mtime),
            )
            .await;
            if f.id == file.id {
                mine = digest;
            } else if let Some(d) = digest {
                others.push(d);
            }
        }
        if let Some(mine) = mine {
            if others.contains(&mine) {
                return Err(refuse(
                    "this file is byte-identical to another copy on the same issue — it is a duplicate, not a stray. Delete the redundant copy instead of moving it; moving it would mark the target issue owned while it holds this issue's content".to_owned(),
                ));
            }
        }
    }

    // The client's target must be independently corroborated by the file's
    // own filename. This is the check that makes a blindly-POSTed issue_id
    // harmless.
    let Some(claimed) = parse_filename_number(&file.path_relative, &patterns) else {
        return Err(refuse(
            "cannot parse an issue number from this file's name — refusing to re-point on a number we can't confirm".to_owned(),
        ));
    };
    if norm_num(&claimed) != norm_num(&to_issue.number) {
        return Err(refuse(format!(
            "filename parses to issue {claimed}, but the target issue is {} — refusing",
            to_issue.number
        )));
    }

    // The target must not already hold a present file with a *different*
    // number: moving in would tangle a second mismatch.
    let occupants: Vec<Option<String>> = file_repo::list_by_issue(&state.db, to_issue.id)
        .await?
        .iter()
        .filter(|f| f.is_present && f.id != file.id)
        .map(|f| parse_filename_number(&f.path_relative, &patterns))
        .collect();
    if !target_accepts(&claimed, &occupants) {
        return Err(refuse(
            "target issue already holds a present file for a different issue number — resolve that mismatch first".to_owned(),
        ));
    }

    // Write only the match columns, and only if the row still points where we
    // validated it. The scanner writes `files` rows outside our lock, so a
    // whole-row write-back from the snapshot we read at the top could silently
    // revert a concurrent scan's disk-state columns.
    let now = crate::routes::reconcile::utc_now();
    let swapped =
        file_repo::repoint_manual(&state.db, file.id, from_issue_id, to_issue.id, now).await?;
    if !swapped {
        return Err(refuse(
            "this file was re-matched while you were deciding — reload and try again".to_owned(),
        ));
    }

    tracing::info!(
        file_id = file.id,
        path = %file.path_relative,
        from_issue_id,
        to_issue_id = to_issue.id,
        "duplicate-files: re-pointed mismatched file"
    );

    Ok(Json(CorrectResult {
        file_id: file.id,
        from_issue_id,
        to_issue_id: to_issue.id,
    }))
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

/// This file's own issue number, from its **filename alone**. `None` when the
/// name yields nothing — which forces the group to `mismatch` (we won't delete
/// on a number we can't confirm).
///
/// The one and only authority for what a file *is*, everywhere in this module:
/// the group classifier that gates deletion, the resolver's re-validation, and
/// the suggested corrections. They must not be able to disagree.
///
/// It used to fall back to the cached ComicInfo `<Number>` when the filename
/// didn't parse — i.e. to the exact source this whole feature exists because it
/// lies. That was a live data-loss path: a scene-named file (`Foo.2025.002.cbz`,
/// which the non-normalizing parser can't read) whose `<Number>` wrongly says 1
/// would report "1", agree with the real issue-1 file it had been mis-matched
/// onto, and the group would present as a deletable `duplicate` — offering to
/// permanently delete one of two *distinct comics*. Two distinct issues can
/// look like duplicates only if we ask a liar to identify them.
///
/// Uses the scanner's Tier-3 normalizing cascade, so this module's idea of a
/// file's number is the same one a future scan will re-derive.
fn parse_filename_number(path_relative: &str, patterns: &[ParsingPattern]) -> Option<String> {
    let basename = path_relative.rsplit('/').next().unwrap_or(path_relative);
    parse_filename_with_normalization(basename, patterns).map(|p| p.number)
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
            suggested_issue_id: None,
            is_served: false,
            suspect_corrupt: suspect,
            under_unsorted: unsorted,
            missing_target_number: None,
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

    // -------- suggested corrections --------

    /// The real Ferocious series: issue rows #1–#5 exist and are distinct.
    fn ferocious_issues() -> Vec<(String, i64)> {
        (1..=5).map(|n| (n.to_string(), 8467 + n)).collect()
    }

    fn nums(v: &[Option<&str>]) -> Vec<Option<String>> {
        v.iter().map(|n| n.map(str::to_owned)).collect()
    }

    #[test]
    fn proposes_the_issue_row_the_filename_names() {
        // The live case: files for #1, #2 and #5 all sitting on issue #1's row
        // (id 8468) because #2's and #5's embedded <Number> both say 1.
        let targets = propose_targets(
            8468,
            &nums(&[Some("1"), Some("2"), Some("5")]),
            &ferocious_issues(),
        );
        // #1's file is already home — nothing to correct.
        assert_eq!(targets[0], None);
        assert_eq!(targets[1], Some(8469));
        assert_eq!(targets[2], Some(8472));
    }

    #[test]
    fn leading_zeros_still_find_the_target() {
        let targets = propose_targets(8468, &nums(&[Some("002")]), &ferocious_issues());
        assert_eq!(targets[0], Some(8469));
    }

    #[test]
    fn no_proposal_when_two_files_claim_the_same_number() {
        // Which of these two IS issue #2? We can't know — both stay manual.
        let targets = propose_targets(
            8468,
            &nums(&[Some("1"), Some("2"), Some("2")]),
            &ferocious_issues(),
        );
        assert_eq!(targets[1], None);
        assert_eq!(targets[2], None);
    }

    #[test]
    fn no_proposal_when_the_target_issue_row_does_not_exist() {
        // Series only has #1–#5; a file parsing to #9 has nowhere to go.
        let targets = propose_targets(8468, &nums(&[Some("9")]), &ferocious_issues());
        assert_eq!(targets[0], None);
    }

    #[test]
    fn no_proposal_when_the_filename_yields_no_number() {
        let targets = propose_targets(8468, &nums(&[None]), &ferocious_issues());
        assert_eq!(targets[0], None);
    }

    #[test]
    fn no_proposal_when_the_series_has_duplicate_rows_for_that_number() {
        // Two catalog rows both numbered "2" — pick one and we may pick wrong.
        let issues = vec![
            ("1".to_owned(), 8468),
            ("2".to_owned(), 8469),
            ("02".to_owned(), 9999),
        ];
        let targets = propose_targets(8468, &nums(&[Some("2")]), &issues);
        assert_eq!(targets[0], None);
    }

    #[test]
    fn target_accepts_an_empty_or_agreeing_issue_but_not_a_conflicting_one() {
        // Nothing there → fine.
        assert!(target_accepts("2", &[]));
        // Occupant agrees on the number → re-pointing makes an honest
        // duplicate group, which the delete resolver then handles.
        assert!(target_accepts("2", &nums(&[Some("002")])));
        // Occupant is a different issue → target is itself mismatched.
        assert!(!target_accepts("2", &nums(&[Some("3")])));
        // Occupant's number is unknown → can't clear it, so don't.
        assert!(!target_accepts("2", &nums(&[None])));
    }

    #[test]
    fn suggest_keep_falls_back_to_largest_when_all_suspect() {
        let files = vec![
            cand(1, "x/a.cbz", 500, true, false),
            cand(2, "x/b.cbz", 900, true, false),
        ];
        assert_eq!(suggest_keep(&files), Some(2));
    }

    // -------- cross-folder guard --------

    fn row(file_id: i64, path: &str) -> DuplicateFileCandidate {
        DuplicateFileCandidate {
            issue_id: 500,
            series_id: 1076,
            series_title: "The Authority".to_owned(),
            issue_number: "4".to_owned(),
            file_id,
            path_relative: path.to_owned(),
            size_bytes: 50_000_000,
            status: "owned".to_owned(),
            library_root_id: 1,
            // Unused by build_one_group — it takes validated digests as a
            // separate argument, precisely so classification can't read an
            // unvalidated stamp straight off the row.
            content_blake3: None,
            hashed_size_bytes: None,
            hashed_mtime: None,
        }
    }

    /// n files that are byte-identical.
    fn same(n: usize) -> Vec<Option<String>> {
        vec![Some("same-digest".to_owned()); n]
    }

    /// n files with distinct content.
    fn differing(n: usize) -> Vec<Option<String>> {
        (0..n).map(|i| Some(format!("digest-{i}"))).collect()
    }

    // -------- content identity --------

    #[test]
    fn identical_digests_are_a_duplicate_and_differing_ones_are_not() {
        assert_eq!(classify_content(&same(2)), ContentVerdict::Identical);
        assert_eq!(classify_content(&same(3)), ContentVerdict::Identical);
        assert_eq!(classify_content(&differing(2)), ContentVerdict::Distinct);
        assert_eq!(classify_content(&differing(3)), ContentVerdict::Distinct);
    }

    /// Absence of a digest is not evidence of sameness OR difference. This is
    /// the same principle as the stale-digest guard: no evidence must never
    /// read as permission to delete.
    #[test]
    fn a_missing_digest_makes_the_whole_group_unknown() {
        assert_eq!(classify_content(&[None, None]), ContentVerdict::Unknown);
        // One unhashed file among matching ones is still Unknown — partial
        // evidence is not partial permission. The unhashed file might be a
        // distinct issue, and deleting it would lose a comic.
        let partial = vec![
            Some("d".to_owned()),
            Some("d".to_owned()),
            None,
        ];
        assert_eq!(classify_content(&partial), ContentVerdict::Unknown);
        // Fewer than two files is nothing to compare.
        assert_eq!(classify_content(&same(1)), ContentVerdict::Unknown);
    }

    /// The live Hello Darkness case: byte-identical files whose filenames
    /// parse to DIFFERENT issue numbers. Under the old filename-number
    /// classifier this was `mismatch`, so Tidy offered to "move" them to
    /// issues #1 and #2 — marking #2 owned while it holds #1's content.
    #[test]
    fn identical_bytes_beat_disagreeing_filenames() {
        let pats = number_pattern();
        let rows = vec![
            row(1, "Hello Darkness/Hello Darkness 001.cbz"),
            row(2, "Hello Darkness/Hello Darkness 002.cbz"),
        ];
        let g = build_one_group(&rows, &pats, &same(2));
        let parsed: Vec<_> = g.files.iter().map(|f| f.parsed_number.clone()).collect();
        assert_eq!(
            parsed,
            vec![Some("001".to_owned()), Some("002".to_owned())],
            "filenames must genuinely disagree, or this proves nothing"
        );
        assert_eq!(g.kind, "duplicate", "identical bytes are a duplicate");
        assert!(g.suggested_keep_file_id.is_some());
    }

    /// The inverse: filenames agree but the bytes differ. Agreement on a name
    /// is not evidence about content.
    #[test]
    fn differing_bytes_beat_agreeing_filenames() {
        let pats = number_pattern();
        let rows = vec![
            row(1, "Saga (2012)/Saga 001.cbz"),
            row(2, "Saga (2012)/Saga 001 alt.cbz"),
        ];
        let g = build_one_group(&rows, &pats, &differing(2));
        assert_eq!(g.kind, "mismatch");
        assert_eq!(g.suggested_keep_file_id, None);
    }

    #[test]
    fn an_unanalyzed_group_is_pending_not_deletable() {
        let pats = number_pattern();
        let rows = vec![
            row(1, "Saga (2012)/Saga 001.cbz"),
            row(2, "Saga (2012)/Saga 001 copy.cbz"),
        ];
        let g = build_one_group(&rows, &pats, &[None, None]);
        assert_eq!(g.kind, "pending_analysis");
        assert_eq!(
            g.suggested_keep_file_id, None,
            "an unanalyzed group must never pre-select a keep"
        );
    }

    #[test]
    fn top_level_folder_extraction() {
        assert_eq!(top_level_folder("Saga (2012)/Saga 001.cbz"), "Saga (2012)");
        assert_eq!(top_level_folder("a/b/c.cbz"), "a");
        // No folder at all — its own location, never equal to a real folder.
        assert_eq!(top_level_folder("loose.cbz"), "");
    }

    /// The live failure this guard exists for. Both files parse to issue 4,
    /// both are bound to the 1999 series' issue #4, and they are different
    /// comics. Number agreement is what makes it dangerous, so the group must
    /// be refused on folder evidence alone.
    #[test]
    fn two_volumes_of_one_title_are_never_a_deletable_duplicate() {
        let rows = vec![
            row(1, "The Authority (1999)/The Authority (1999) 004.cbz"),
            row(2, "The Authority (2008)/The Authority 004 (2009) (Digital).cbr"),
        ];
        let g = build_one_group(&rows, &[], &same(2));
        assert_eq!(g.kind, "cross_folder_wrong_series");
        assert_eq!(
            g.suggested_keep_file_id, None,
            "a cross-folder group must never pre-select a file to keep"
        );
    }

    #[test]
    fn same_series_under_two_folder_names_is_labelled_as_such() {
        // One series, two folder spellings — benign, but still not deletable.
        let rows = vec![
            row(1, "Drifter/Drifter 002.cbz"),
            row(2, "Drifter (2014)/Drifter 002.cbz"),
        ];
        let g = build_one_group(&rows, &[], &same(2));
        assert_eq!(g.kind, "cross_folder_same_series");
        assert_eq!(g.suggested_keep_file_id, None);
    }

    #[test]
    fn leading_article_difference_still_reads_as_one_series() {
        let rows = vec![
            row(1, "The Holy Roller/x 001.cbz"),
            row(2, "Holy Roller (2024)/x 001.cbz"),
        ];
        assert_eq!(
            build_one_group(&rows, &[], &same(2)).kind,
            "cross_folder_same_series"
        );
    }

    /// A number-parsing pattern, so the duplicate path can actually be
    /// reached — with no patterns nothing parses and every group degrades to
    /// `mismatch`, which would make a "still a duplicate" assertion vacuous.
    fn number_pattern() -> Vec<ParsingPattern> {
        vec![ParsingPattern {
            id: 1,
            name: "test".to_owned(),
            pattern: r"^(?P<series>[A-Za-z ]+?)\s+(?P<number>\d+).*\.(?i:cbz|cbr)$".to_owned(),
            priority: 1,
            enabled: true,
        }]
    }

    #[test]
    fn a_genuine_single_folder_duplicate_still_resolves_normally() {
        // The guard must not swallow the case the feature exists to handle.
        let pats = number_pattern();
        let rows = vec![
            row(1, "Saga (2012)/Saga 001.cbz"),
            row(2, "Saga (2012)/Saga 001 copy.cbz"),
        ];
        let g = build_one_group(&rows, &pats, &same(2));
        assert_eq!(
            g.files.iter().filter(|f| f.parsed_number.is_some()).count(),
            2,
            "both filenames must parse, or this test proves nothing"
        );
        assert_eq!(g.kind, "duplicate");
        assert!(g.suggested_keep_file_id.is_some());
    }

    /// Number agreement must not rescue a cross-folder group — this is the
    /// precise combination that made the Authority groups look deletable.
    #[test]
    fn agreeing_numbers_do_not_override_the_folder_guard() {
        let pats = number_pattern();
        let rows = vec![
            row(1, "Authority A/Authority 004.cbz"),
            row(2, "Authority B/Authority 004.cbz"),
        ];
        let g = build_one_group(&rows, &pats, &same(2));
        assert_eq!(
            g.files.iter().filter(|f| f.parsed_number.is_some()).count(),
            2,
            "both numbers parse and agree"
        );
        assert!(
            g.kind.starts_with("cross_folder"),
            "agreeing numbers must not make a cross-folder group deletable, got {}",
            g.kind
        );
        assert_eq!(g.suggested_keep_file_id, None);
    }

    #[test]
    fn folder_identity_splits_title_from_volume_year() {
        assert_eq!(folder_identity("Drifter (2014)"), ("drifter".into(), Some(2014)));
        assert_eq!(folder_identity("Drifter"), ("drifter".into(), None));
        assert_eq!(
            folder_identity("The Authority (1999)"),
            ("authority".into(), Some(1999))
        );
        // Parenthesised non-year stays part of the title.
        assert_eq!(
            folder_identity("Ultramega (Deluxe)"),
            ("ultramega deluxe".into(), None)
        );
    }

    #[test]
    fn same_title_different_years_is_not_the_same_series() {
        // The inversion this test guards: strip the year naively and these
        // two normalize identically, turning the single most dangerous case
        // into a reassuring "same series" label.
        assert!(!folders_agree_on_series(&[
            "The Authority (1999)",
            "The Authority (2008)"
        ]));
        // A missing year is compatible with any year — one folder simply
        // wasn't named with one.
        assert!(folders_agree_on_series(&["Drifter", "Drifter (2014)"]));
    }
}
