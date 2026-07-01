//! Library Tidy reconciliation routes — surface and resolve the two
//! kinds of catalog/disk drift: phantom series (catalog tracks a series,
//! disk has no files for it) and untracked folders (disk has a
//! series-shaped folder the catalog doesn't know).
//!
//! Phantom deletes route through `series::delete_series` so the
//! owned-files guard is identical to `DELETE /api/series/:id`. The
//! single-delete endpoint is strict (404/409); the batch endpoints
//! (`add`, `phantoms/bulk`) are best-effort and report per-row
//! outcomes — a bulk action over a stale tidy view will routinely
//! include rows that have since changed, and one stale row shouldn't
//! sink the whole batch.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use longbox_core::{normalize_title, parse_filename, FileStatus, MatchMethod, ParsingPattern};
use longbox_db::{
    discovered_folders_repo, file_repo, issue_repo, library_root_repo, parsing_pattern_repo,
    series_repo, DbError, DiscoveredFolderRow, DuplicatePair, FileRow, FileUpdate, NewSeries,
    PhantomSeries,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::error::ApiError;
use crate::routes::series::{add_or_get_from_cv, delete_series, spawn_auto_rematch};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/reconcile/phantoms", get(phantoms))
        .route("/reconcile/phantoms/kept", get(kept_phantoms))
        .route("/reconcile/untracked", get(untracked))
        .route("/reconcile/counts", get(counts))
        .route("/reconcile/add", post(add))
        .route("/reconcile/dismiss", post(dismiss))
        .route("/reconcile/phantom/:series_id", delete(delete_phantom))
        .route(
            "/reconcile/phantom/:series_id/keep",
            post(keep_phantom).delete(unkeep_phantom),
        )
        .route("/reconcile/phantoms/bulk", post(bulk_delete_phantoms))
        .route("/reconcile/convert", post(convert))
        .route("/library/tidy/duplicates", get(duplicates))
        .route("/library/tidy/duplicates/merge", post(merge_duplicates))
}

// -------- shapes --------

/// Both phantom surfaces in one payload. `with_transition` is the subset
/// of `all_zero_owned` with `last_matched_count > 0` — series that held
/// files at the last scan and have since lost them all (the strong
/// just-deleted-a-folder signal). `all_zero_owned` is every zero-owned
/// series, transition rows included; the UI renders it as the full
/// catalog-hygiene list.
#[derive(Debug, Serialize)]
struct PhantomsResponse {
    with_transition: Vec<PhantomSeries>,
    all_zero_owned: Vec<PhantomSeries>,
}

/// Lightweight reconciliation counts for the dashboard banner — just
/// the two numbers, so the landing page never pulls the full phantom
/// or untracked lists.
#[derive(Debug, Serialize)]
struct ReconcileCounts {
    phantoms_with_transition: i64,
    untracked_folders: i64,
}

#[derive(Debug, Deserialize)]
struct AddBody {
    folders: Vec<AddFolder>,
}

#[derive(Debug, Deserialize)]
struct AddFolder {
    folder_name: String,
    cv_id: i64,
}

#[derive(Debug, Serialize)]
struct AddResponse {
    succeeded: Vec<AddSucceeded>,
    failed: Vec<AddFailed>,
}

#[derive(Debug, Serialize)]
struct AddSucceeded {
    folder_name: String,
    series_id: i64,
}

#[derive(Debug, Serialize)]
struct AddFailed {
    folder_name: String,
    error: String,
}

#[derive(Debug, Deserialize)]
struct DismissBody {
    folder_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BulkDeleteBody {
    series_ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
struct BulkDeleteResponse {
    deleted: Vec<i64>,
    skipped: Vec<SkippedSeries>,
}

#[derive(Debug, Serialize)]
struct SkippedSeries {
    series_id: i64,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct ConvertBody {
    folder_names: Vec<String>,
}

/// Per-folder outcome of a bulk shallow-convert. `status` is `added`
/// (the folder became a new tracked series), `linked` (the folder
/// matched an existing series on `(sort_title, start_year)` and its
/// files were attached to that survivor — A.9 hot-fix), or `failed`.
/// `series_id` rides along on success, `error` on failure.
#[derive(Debug, Serialize)]
struct ConvertResult {
    folder_name: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    series_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConvertResponse {
    results: Vec<ConvertResult>,
}

// -------- handlers --------

/// Both phantom surfaces. `list_phantoms` returns every zero-owned
/// series in one query; the transition/steady-state split is this
/// route's job (a single filter pass), not the repo's.
async fn phantoms(State(state): State<AppState>) -> Result<Json<PhantomsResponse>, ApiError> {
    let all_zero_owned = series_repo::list_phantoms(&state.db).await?;
    let with_transition = all_zero_owned
        .iter()
        .filter(|p| p.last_matched_count > 0)
        .cloned()
        .collect();
    Ok(Json(PhantomsResponse {
        with_transition,
        all_zero_owned,
    }))
}

/// The "Kept series" surface: phantoms the user explicitly exempted from
/// tidy. Filtered out of `phantoms` above, listed here so the UI can
/// render its own collapsed section with an "Un-keep" action.
async fn kept_phantoms(
    State(state): State<AppState>,
) -> Result<Json<Vec<PhantomSeries>>, ApiError> {
    Ok(Json(series_repo::list_kept_series(&state.db).await?))
}

/// Non-dismissed discovered folders — the untracked working set.
async fn untracked(
    State(state): State<AppState>,
) -> Result<Json<Vec<DiscoveredFolderRow>>, ApiError> {
    Ok(Json(discovered_folders_repo::list(&state.db).await?))
}

/// Reconciliation counts for the dashboard banner. Reuses the list
/// repos and counts in Rust rather than running dedicated `COUNT`
/// queries — the phantom and untracked sets are small enough that the
/// extra rows are cheap, and it keeps this endpoint free of new SQL.
async fn counts(State(state): State<AppState>) -> Result<Json<ReconcileCounts>, ApiError> {
    let phantoms = series_repo::list_phantoms(&state.db).await?;
    let untracked = discovered_folders_repo::list(&state.db).await?;
    // A series already marked for automatic removal (`auto_tidy_due_at`)
    // is handled, not pending — exclude it from the dashboard's
    // attention count even though it still reads as a transition phantom.
    let phantoms_with_transition = phantoms
        .iter()
        .filter(|p| p.last_matched_count > 0 && p.auto_tidy_due_at.is_none())
        .count() as i64;
    Ok(Json(ReconcileCounts {
        phantoms_with_transition,
        untracked_folders: untracked.len() as i64,
    }))
}

/// Add one or more discovered folders to the catalog. Per-row
/// best-effort: each folder's ComicVine fetch can fail independently
/// (rate limit, unknown volume, network), so one failure never aborts
/// the batch — the response splits outcomes into `succeeded` / `failed`.
async fn add(
    State(state): State<AppState>,
    Json(body): Json<AddBody>,
) -> Result<Json<AddResponse>, ApiError> {
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    for folder in body.folders {
        match add_one(&state, &folder).await {
            Ok(series_id) => succeeded.push(AddSucceeded {
                folder_name: folder.folder_name,
                series_id,
            }),
            Err(e) => failed.push(AddFailed {
                folder_name: folder.folder_name,
                error: e.to_string(),
            }),
        }
    }
    Ok(Json(AddResponse { succeeded, failed }))
}

/// Resolve one discovered folder against ComicVine and fold it into the
/// catalog. Idempotent on the series — `add_or_get_from_cv` returns the
/// existing row if `cv_id` is already tracked. The `discovered_folders`
/// row is dismissed either way: once a series owns the `cv_id`, the
/// folder is accounted for. The dismiss is by folder name and itself
/// idempotent, so a name matching no open row is a clean no-op.
async fn add_one(state: &AppState, folder: &AddFolder) -> Result<i64, ApiError> {
    if folder.cv_id <= 0 {
        return Err(ApiError::BadRequest {
            message: "cv_id must be > 0".into(),
        });
    }
    let (series, _was_new) = add_or_get_from_cv(state, folder.cv_id).await?;
    // Auto-dismiss: the folder is now tracked, but if the user later
    // removes the series the folder must resurface — system action,
    // not user-permanent.
    discovered_folders_repo::auto_dismiss(&state.db, std::slice::from_ref(&folder.folder_name))
        .await?;
    // The folder holds CBZs that didn't resolve to any tracked series;
    // rematch now so they attach to the series we just added instead of
    // it sitting as a phantom until the next full scan.
    spawn_auto_rematch(state, series.id, "reconcile-add");
    Ok(series.id)
}

/// Bulk-dismiss discovered folders the user doesn't want to track.
/// `dismiss` is idempotent; the count is rows *newly* dismissed.
async fn dismiss(
    State(state): State<AppState>,
    Json(body): Json<DismissBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dismissed = discovered_folders_repo::dismiss(&state.db, &body.folder_names).await?;
    Ok(Json(serde_json::json!({ "dismissed": dismissed })))
}

/// Strict single phantom delete: 404 for an unknown series, 409 when the
/// series still owns files. Routes through the shared `delete_series`
/// guard.
async fn delete_phantom(
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    delete_series(&state.db, series_id).await?;
    Ok(Json(serde_json::json!({ "deleted": series_id })))
}

/// "Keep" a phantom: set the durable `tidy_exempt` flag (and clear the
/// auto-tidy signals). The series leaves the phantom list entirely and
/// the scanner never re-marks it — it moves to the "Kept series" surface,
/// reversible via `DELETE` (`unkeep_phantom`). The user has reviewed the
/// series and decided it stays; the same action serves the "recently
/// lost files" and "scheduled for removal" tidy surfaces. 404 for an
/// unknown series; the explicit existence check gives a clean `series`
/// 404 rather than the generic `row` one `keep_phantom_series`'s own
/// `NotFound` would map to.
async fn keep_phantom(
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if series_repo::find_by_id(&state.db, series_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "series",
            id: series_id.to_string(),
        });
    }
    series_repo::keep_phantom_series(&state.db, series_id).await?;
    Ok(Json(serde_json::json!({ "kept": series_id })))
}

/// Reverse a "Keep": clear the tidy exemption so the scanner
/// re-evaluates the series normally. 404 for an unknown series (same
/// explicit existence check as `keep_phantom`, for a clean `series` 404).
async fn unkeep_phantom(
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if series_repo::find_by_id(&state.db, series_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "series",
            id: series_id.to_string(),
        });
    }
    series_repo::unkeep_phantom_series(&state.db, series_id).await?;
    Ok(Json(serde_json::json!({ "unkept": series_id })))
}

/// Best-effort bulk phantom delete. Each id routes through the shared
/// `delete_series` guard; an unknown or no-longer-phantom series is
/// reported in `skipped` rather than failing the request.
async fn bulk_delete_phantoms(
    State(state): State<AppState>,
    Json(body): Json<BulkDeleteBody>,
) -> Result<Json<BulkDeleteResponse>, ApiError> {
    let mut deleted = Vec::new();
    let mut skipped = Vec::new();
    for series_id in body.series_ids {
        match delete_series(&state.db, series_id).await {
            Ok(()) => deleted.push(series_id),
            Err(e) => skipped.push(SkippedSeries {
                series_id,
                reason: e.to_string(),
            }),
        }
    }
    Ok(Json(BulkDeleteResponse { deleted, skipped }))
}

/// Bulk shallow-convert: turn discovered folders into tracked series
/// without ComicVine. Each selected folder either becomes a NEW series
/// (status `added`) or — when an existing series matches on
/// `(sort_title, start_year)` (A.9 hot-fix idempotency) — has its
/// files attached to that survivor (status `linked`). Either way,
/// parsed-number issue rows are synthesized as needed and files
/// attach as `owned`/`FilenameRegex`/confidence 1.0 — the only
/// attachment shape stable against the next scan's cascade-revert
/// when a CV-tracked rival exists. No CV metadata; covers and the
/// canonical issue list arrive later via the enrichment step.
/// Per-folder atomic; the batch is non-transactional, so one bad
/// folder never sinks the rest.
async fn convert(
    State(state): State<AppState>,
    Json(body): Json<ConvertBody>,
) -> Result<Json<ConvertResponse>, ApiError> {
    let patterns = load_patterns(&state).await?;

    // Every file, grouped by top-level folder — one query set reused
    // across every folder in the batch.
    let mut by_folder: HashMap<String, Vec<FileRow>> = HashMap::new();
    for root in library_root_repo::list_all(&state.db).await? {
        for file in file_repo::list_by_library_root(&state.db, root.id).await? {
            if let Some(folder) = top_level_folder(&file.path_relative) {
                by_folder.entry(folder.to_owned()).or_default().push(file);
            }
        }
    }

    let mut results = Vec::with_capacity(body.folder_names.len());
    for folder_name in body.folder_names {
        let files: &[FileRow] = by_folder
            .get(&folder_name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let result = match convert_one_folder(&state, &folder_name, files, &patterns).await {
            Ok((series_id, status)) => ConvertResult {
                folder_name,
                status,
                series_id: Some(series_id),
                error: None,
            },
            Err(e) => ConvertResult {
                folder_name,
                status: "failed",
                series_id: None,
                error: Some(e.to_string()),
            },
        };
        results.push(result);
    }
    Ok(Json(ConvertResponse { results }))
}

/// Convert one folder, atomically. Either matches an existing series
/// on `(sort_title, start_year)` and links the folder to it (status
/// `linked`), or inserts a new shallow series (status `added`). In
/// both branches: for every parsed file number, ensure an issue row
/// exists on the survivor (synthesized as number-only when missing;
/// pre-existing CV-canonical issues preserved as-is), then attach
/// each folder file to its resolved issue as
/// `owned`/`FilenameRegex`/confidence 1.0, and dismiss the
/// discovered-folders row. Any failure rolls the whole folder back.
///
/// The link mode is the A.9 hot-fix: the previous code inserted a
/// shallow series unconditionally, creating duplicates whenever a
/// CV-tracked (or earlier-shallow-converted) series with the same
/// title and launch year already existed.
async fn convert_one_folder(
    state: &AppState,
    folder_name: &str,
    files: &[FileRow],
    patterns: &[ParsingPattern],
) -> Result<(i64, &'static str), ApiError> {
    let (title, start_year) = parse_folder_name(folder_name);
    let sort_title = normalize_title(&title);

    let mut tx = state.db.begin().await.map_err(longbox_db::DbError::from)?;

    // Idempotency on (sort_title, start_year). NULL-safe match handles
    // Pattern C (folders lacking `(YYYY)` whose start_year parses to
    // None) — those rows must dedup against each other.
    let (series_id, status) =
        match series_repo::find_for_dedup(&mut *tx, &sort_title, start_year).await? {
            Some(existing) => (existing.id, "linked"),
            None => {
                let inserted = series_repo::insert(
                    &mut *tx,
                    NewSeries {
                        cv_id: None,
                        metron_id: None,
                        sort_title,
                        title,
                        start_year,
                        publisher: None,
                        description: None,
                        cover_url: None,
                    },
                )
                .await?;
                (inserted.id, "added")
            }
        };

    // Parse an issue number out of each file's basename. A file whose
    // name doesn't parse is left untouched — it stays unmatched.
    let parsed: Vec<(i64, String)> = files
        .iter()
        .filter_map(|f| {
            let basename = f.path_relative.rsplit('/').next()?;
            let p = parse_filename(basename, patterns)?;
            Some((f.id, p.number))
        })
        .collect();

    // Upsert each distinct parsed number as an issue on the survivor.
    // `upsert_number_only_returning` synthesizes only the numbers the
    // survivor doesn't already have — pre-existing CV-canonical issues
    // for matching numbers stay authoritative.
    let mut numbers: Vec<String> = parsed.iter().map(|(_, n)| n.clone()).collect();
    numbers.sort();
    numbers.dedup();
    let issue_by_number: HashMap<String, i64> =
        issue_repo::upsert_number_only_returning(&mut *tx, series_id, &numbers)
            .await?
            .into_iter()
            .map(|(id, n)| (n, id))
            .collect();

    // Attach each parsed file to its resolved issue as an owned file.
    let now = utc_now();
    let mut attached = 0u32;
    for f in files {
        let Some((_, number)) = parsed.iter().find(|(id, _)| *id == f.id) else {
            continue;
        };
        let Some(&issue_id) = issue_by_number.get(number) else {
            continue;
        };
        let patch = FileUpdate {
            issue_id: Some(issue_id),
            size_bytes: f.size_bytes,
            mtime: f.mtime,
            last_scanned_at: f.last_scanned_at,
            match_method: MatchMethod::FilenameRegex.as_db_str().to_owned(),
            match_confidence: 1.0,
            status: FileStatus::Owned.as_db_str().to_owned(),
            cached_comicinfo_xml: f.cached_comicinfo_xml.clone(),
            cached_at: f.cached_at,
            is_present: f.is_present,
            last_seen_at: f.last_seen_at,
            matched_at: file_repo::next_matched_at(f.issue_id, Some(issue_id), f.matched_at, now),
        };
        file_repo::update(&mut *tx, f.id, patch).await?;
        attached += 1;
    }

    // A.9 Bug 1a — zero-attachment rollback. If the attach loop didn't
    // bind a single file to an issue, the convert is structurally
    // broken: a series row would exist with no owned files and the
    // discovered folder would auto-dismiss, hiding the un-tracked
    // problem rather than solving it. Dropping `tx` without commit
    // undoes the series insert and any synthesized issue rows; the
    // folder stays visible in /library/tidy. The user gets a clear
    // signal that none of the folder's filenames parsed.
    if attached == 0 {
        return Err(ApiError::Unprocessable {
            code: "unprocessable.no_parseable_files",
            message: format!(
                "0 of {} files in '{}' have parseable issue numbers; no series created and folder remains untracked",
                files.len(),
                folder_name,
            ),
        });
    }

    // Auto-dismiss: bulk-convert tracks the folder, but if the user
    // later removes the series the folder must resurface — system
    // action, not user-permanent.
    discovered_folders_repo::auto_dismiss(&mut *tx, &[folder_name.to_owned()]).await?;

    tx.commit().await.map_err(longbox_db::DbError::from)?;
    Ok((series_id, status))
}

/// Split a discovered-folder name into (title, start_year). A trailing
/// ` (YYYY)` is read as the launch year; a folder without one keeps the
/// whole name as the title.
fn parse_folder_name(folder: &str) -> (String, Option<i32>) {
    let trimmed = folder.trim();
    if let Some(inner) = trimmed.strip_suffix(')') {
        if let Some((title, year)) = inner.rsplit_once(" (") {
            let title = title.trim();
            if year.len() == 4 && !title.is_empty() {
                if let Ok(y) = year.parse::<i32>() {
                    return (title.to_owned(), Some(y));
                }
            }
        }
    }
    (trimmed.to_owned(), None)
}

/// First path component of a forward-slash relative path, or `None` for
/// a file loose in the library root.
fn top_level_folder(path_relative: &str) -> Option<&str> {
    let (first, rest) = path_relative.split_once('/')?;
    (!rest.is_empty()).then_some(first)
}

/// Load the enabled parsing patterns into `longbox-core`'s shape — the
/// same mapping the scanner does.
async fn load_patterns(state: &AppState) -> Result<Vec<ParsingPattern>, ApiError> {
    let rows = parsing_pattern_repo::list_enabled(&state.db).await?;
    Ok(rows
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

fn utc_now() -> PrimitiveDateTime {
    let n = OffsetDateTime::now_utc();
    PrimitiveDateTime::new(n.date(), n.time())
}

// -------- Library Tidy: duplicates --------

#[derive(Debug, Serialize)]
struct DuplicatesResponse {
    pairs: Vec<DuplicatePair>,
}

/// `GET /api/library/tidy/duplicates` — list every pair of series the
/// detection criteria flag as potential duplicates (same cv_id, OR
/// same normalized title with start_year within 2 years). Owned-and-
/// present file counts ride along so the UI can render "merge smaller
/// into larger" without a follow-up roundtrip.
async fn duplicates(State(state): State<AppState>) -> Result<Json<DuplicatesResponse>, ApiError> {
    let pairs = series_repo::find_duplicate_pairs(&state.db).await?;
    Ok(Json(DuplicatesResponse { pairs }))
}

#[derive(Debug, Deserialize)]
struct MergeDuplicatesBody {
    target_series_id: i64,
    source_series_id: i64,
}

#[derive(Debug, Serialize)]
struct MergeDuplicatesResponse {
    target_series_id: i64,
    source_series_id: i64,
}

/// `POST /api/library/tidy/duplicates/merge` — move every issue (and,
/// transitively, every file linked through those issues) from
/// `source_series_id` to `target_series_id`, then hard-delete the
/// emptied source row. Caller decides which series is the "larger"
/// (target) and which is the "smaller" (source); the backend does not
/// re-derive owned counts here.
///
/// The merge runs in a single transaction so a failure mid-step rolls
/// the catalog back to its pre-merge state — never a half-merged
/// half-deleted situation. 400 when target == source. 404 when either
/// id doesn't resolve to a series row.
async fn merge_duplicates(
    State(state): State<AppState>,
    Json(body): Json<MergeDuplicatesBody>,
) -> Result<Json<MergeDuplicatesResponse>, ApiError> {
    if body.target_series_id == body.source_series_id {
        return Err(ApiError::BadRequest {
            message: "target_series_id and source_series_id must differ".into(),
        });
    }
    // Existence preflight gives a clean 404 on either side instead of
    // a generic "0 rows affected" that the user has to translate.
    if series_repo::find_by_id(&state.db, body.target_series_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "series",
            id: body.target_series_id.to_string(),
        });
    }
    if series_repo::find_by_id(&state.db, body.source_series_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "series",
            id: body.source_series_id.to_string(),
        });
    }

    let mut tx = state.db.begin().await.map_err(DbError::from)?;
    series_repo::merge_series(&mut *tx, body.target_series_id, body.source_series_id).await?;
    series_repo::hard_delete(&mut *tx, body.source_series_id).await?;
    tx.commit().await.map_err(DbError::from)?;

    Ok(Json(MergeDuplicatesResponse {
        target_series_id: body.target_series_id,
        source_series_id: body.source_series_id,
    }))
}
