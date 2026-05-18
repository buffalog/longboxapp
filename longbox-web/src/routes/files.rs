use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_core::{FileStatus, IssueNumber, MatchMethod, ParsingPattern};
use longbox_db::{file_repo, issue_repo, parsing_pattern_repo, FileRow, FileUpdate};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::ApiError;
use crate::routes::series::{add_or_get_from_cv, spawn_auto_rematch};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/files", get(list))
        .route("/files/:id", get(detail).patch(update))
        .route("/files/:id/match-from-cv", post(match_from_cv))
        .route("/files/match-folder-from-cv", post(match_folder_from_cv))
}

#[derive(Debug, Deserialize, Default)]
struct ListParams {
    status: Option<String>,
    library_root_id: Option<i64>,
}

/// File row with the matched issue and its series embedded. Returned by
/// every `/api/files` shape (list, detail, PATCH result). When `issue_id`
/// is null both `issue` and `series` are null.
#[derive(Debug, Serialize)]
struct EnrichedFileRow {
    #[serde(flatten)]
    file: FileRow,
    issue: Option<IssueSnippet>,
    series: Option<SeriesSnippet>,
}

#[derive(Debug, Serialize)]
struct IssueSnippet {
    id: i64,
    number: String,
    title: Option<String>,
    cover_date: Option<String>,
}

#[derive(Debug, Serialize)]
struct SeriesSnippet {
    id: i64,
    title: String,
    start_year: Option<i64>,
}

/// Flat row shape for the JOIN'd files-with-issue-and-series query.
struct FileEnrichedJoinRow {
    id: i64,
    issue_id: Option<i64>,
    library_root_id: i64,
    path_relative: String,
    size_bytes: i64,
    mtime: time::PrimitiveDateTime,
    last_scanned_at: time::PrimitiveDateTime,
    match_method: String,
    match_confidence: f64,
    status: String,
    cached_comicinfo_xml: Option<String>,
    cached_at: Option<time::PrimitiveDateTime>,
    is_present: bool,
    last_seen_at: time::PrimitiveDateTime,
    matched_at: Option<time::PrimitiveDateTime>,
    issue_inner_id: Option<i64>,
    issue_number: Option<String>,
    issue_title: Option<String>,
    issue_cover_date: Option<String>,
    series_inner_id: Option<i64>,
    series_title: Option<String>,
    series_start_year: Option<i64>,
}

fn enrich(r: FileEnrichedJoinRow) -> EnrichedFileRow {
    let file = FileRow {
        id: r.id,
        issue_id: r.issue_id,
        library_root_id: r.library_root_id,
        path_relative: r.path_relative,
        size_bytes: r.size_bytes,
        mtime: r.mtime,
        last_scanned_at: r.last_scanned_at,
        match_method: r.match_method,
        match_confidence: r.match_confidence,
        status: r.status,
        cached_comicinfo_xml: r.cached_comicinfo_xml,
        cached_at: r.cached_at,
        is_present: r.is_present,
        last_seen_at: r.last_seen_at,
        matched_at: r.matched_at,
    };
    let issue = match (r.issue_inner_id, r.issue_number) {
        (Some(id), Some(number)) => Some(IssueSnippet {
            id,
            number,
            title: r.issue_title,
            cover_date: r.issue_cover_date,
        }),
        _ => None,
    };
    let series = match (r.series_inner_id, r.series_title) {
        (Some(id), Some(title)) => Some(SeriesSnippet {
            id,
            title,
            start_year: r.series_start_year,
        }),
        _ => None,
    };
    EnrichedFileRow {
        file,
        issue,
        series,
    }
}

async fn fetch_enriched_by_id(
    pool: &longbox_db::Pool,
    id: i64,
) -> Result<EnrichedFileRow, ApiError> {
    let row = sqlx::query_as!(
        FileEnrichedJoinRow,
        r#"SELECT
             f.id AS "id!: i64",
             f.issue_id,
             f.library_root_id AS "library_root_id!: i64",
             f.path_relative,
             f.size_bytes AS "size_bytes!: i64",
             f.mtime AS "mtime: time::PrimitiveDateTime",
             f.last_scanned_at AS "last_scanned_at: time::PrimitiveDateTime",
             f.match_method, f.match_confidence, f.status,
             f.cached_comicinfo_xml,
             f.cached_at AS "cached_at: time::PrimitiveDateTime",
             f.is_present AS "is_present!: bool",
             f.last_seen_at AS "last_seen_at: time::PrimitiveDateTime",
             f.matched_at AS "matched_at: time::PrimitiveDateTime",
             i.id AS "issue_inner_id?: i64",
             i.number AS "issue_number?",
             i.title AS "issue_title?",
             i.cover_date AS "issue_cover_date?",
             s.id AS "series_inner_id?: i64",
             s.title AS "series_title?",
             s.start_year AS "series_start_year?: i64"
           FROM files f
           LEFT JOIN issues i ON f.issue_id = i.id
           LEFT JOIN series s ON i.series_id = s.id
           WHERE f.id = ?"#,
        id
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("file detail query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?
    .ok_or_else(|| ApiError::NotFound {
        resource: "file",
        id: id.to_string(),
    })?;
    Ok(enrich(row))
}

async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<EnrichedFileRow>>, ApiError> {
    let library_root_id = params
        .library_root_id
        .unwrap_or(state.library_root_id);

    let validated_status = match params.status.as_deref() {
        None | Some("all") => None,
        Some(s) => {
            if FileStatus::from_db_str(s).is_none() {
                return Err(ApiError::BadRequest {
                    message: format!("unknown status: {s:?}"),
                });
            }
            Some(s.to_owned())
        }
    };

    let status_filter = validated_status.as_deref();
    let rows: Vec<FileEnrichedJoinRow> = sqlx::query_as!(
        FileEnrichedJoinRow,
        r#"SELECT
             f.id AS "id!: i64",
             f.issue_id,
             f.library_root_id AS "library_root_id!: i64",
             f.path_relative,
             f.size_bytes AS "size_bytes!: i64",
             f.mtime AS "mtime: time::PrimitiveDateTime",
             f.last_scanned_at AS "last_scanned_at: time::PrimitiveDateTime",
             f.match_method, f.match_confidence, f.status,
             f.cached_comicinfo_xml,
             f.cached_at AS "cached_at: time::PrimitiveDateTime",
             f.is_present AS "is_present!: bool",
             f.last_seen_at AS "last_seen_at: time::PrimitiveDateTime",
             f.matched_at AS "matched_at: time::PrimitiveDateTime",
             i.id AS "issue_inner_id?: i64",
             i.number AS "issue_number?",
             i.title AS "issue_title?",
             i.cover_date AS "issue_cover_date?",
             s.id AS "series_inner_id?: i64",
             s.title AS "series_title?",
             s.start_year AS "series_start_year?: i64"
           FROM files f
           LEFT JOIN issues i ON f.issue_id = i.id
           LEFT JOIN series s ON i.series_id = s.id
           WHERE f.library_root_id = ?
             AND (?2 IS NULL OR f.status = ?2)
           ORDER BY f.path_relative"#,
        library_root_id,
        status_filter
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("file list query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;

    Ok(Json(rows.into_iter().map(enrich).collect()))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<EnrichedFileRow>, ApiError> {
    Ok(Json(fetch_enriched_by_id(&state.db, id).await?))
}

/// PATCH body shapes:
/// - `{ "issue_id": 42 }` — manual rematch: set issue, mark owned/manual.
/// - `{ "status": "ignored" }` — flag as not-a-comic.
/// - `{ "status": null }` — clear ignore: revert to unmatched.
#[derive(Debug, Deserialize)]
struct PatchBody {
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    issue_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    status: Option<Option<String>>,
}

fn deserialize_optional_field<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<PatchBody>,
) -> Result<Json<EnrichedFileRow>, ApiError> {
    let existing = file_repo::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "file",
            id: id.to_string(),
        })?;

    let now = now_utc_primitive();
    let mut patch = FileUpdate {
        issue_id: existing.issue_id,
        size_bytes: existing.size_bytes,
        mtime: existing.mtime,
        last_scanned_at: now,
        match_method: existing.match_method.clone(),
        match_confidence: existing.match_confidence,
        status: existing.status.clone(),
        cached_comicinfo_xml: existing.cached_comicinfo_xml.clone(),
        cached_at: existing.cached_at,
        is_present: existing.is_present,
        last_seen_at: existing.last_seen_at,
        // Placeholder; recomputed below once `patch.issue_id` is final.
        matched_at: existing.matched_at,
    };

    match (&body.issue_id, &body.status) {
        (Some(Some(new_issue_id)), None) => {
            let issue = issue_repo::find_by_id(&state.db, *new_issue_id)
                .await?
                .ok_or_else(|| ApiError::NotFound {
                    resource: "issue",
                    id: new_issue_id.to_string(),
                })?;
            patch.issue_id = Some(issue.id);
            patch.match_method = MatchMethod::Manual.as_db_str().to_owned();
            patch.match_confidence = 1.0;
            patch.status = FileStatus::Owned.as_db_str().to_owned();
        }
        (None, Some(Some(s))) if s == FileStatus::Ignored.as_db_str() => {
            patch.issue_id = None;
            patch.match_method = MatchMethod::Ignored.as_db_str().to_owned();
            patch.match_confidence = 0.0;
            patch.status = FileStatus::Ignored.as_db_str().to_owned();
        }
        (None, Some(None)) => {
            if existing.status != FileStatus::Ignored.as_db_str() {
                return Err(ApiError::BadRequest {
                    message: "cannot clear status of a non-ignored file".into(),
                });
            }
            patch.issue_id = None;
            patch.match_method = MatchMethod::Unmatched.as_db_str().to_owned();
            patch.match_confidence = 0.0;
            patch.status = FileStatus::Unmatched.as_db_str().to_owned();
        }
        (None, None) => {
            return Err(ApiError::BadRequest {
                message: "PATCH body must contain `issue_id` or `status`".into(),
            });
        }
        _ => {
            return Err(ApiError::BadRequest {
                message: "ambiguous PATCH body; use either `issue_id` OR `status`, not both"
                    .into(),
            });
        }
    }

    patch.matched_at =
        file_repo::next_matched_at(existing.issue_id, patch.issue_id, existing.matched_at, now);
    file_repo::update(&state.db, id, patch).await?;
    Ok(Json(fetch_enriched_by_id(&state.db, id).await?))
}

fn now_utc_primitive() -> time::PrimitiveDateTime {
    let n = OffsetDateTime::now_utc();
    time::PrimitiveDateTime::new(n.date(), n.time())
}

#[derive(Debug, Deserialize)]
struct MatchFromCvBody {
    cv_volume_id: i64,
    /// Optional override for the issue number. When omitted, the handler
    /// resolves from the file's cached ComicInfo `<Number>` first, then
    /// falls back to filename parsing with the stored `parsing_patterns`.
    issue_number: Option<String>,
}

/// POST /api/files/:id/match-from-cv
///
/// Adds the CV volume to the watchlist (or reuses an existing series),
/// resolves the issue number, sets the file to manual / owned, and fires
/// a fire-and-forget series-wide rematch (which picks up sibling files in
/// the library that should match the just-added series).
async fn match_from_cv(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<MatchFromCvBody>,
) -> Result<Json<EnrichedFileRow>, ApiError> {
    if body.cv_volume_id <= 0 {
        return Err(ApiError::BadRequest {
            message: "cv_volume_id must be > 0".into(),
        });
    }
    let file = file_repo::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "file",
            id: id.to_string(),
        })?;

    let (series, _was_new) = add_or_get_from_cv(&state, body.cv_volume_id).await?;

    let patterns = load_enabled_patterns(&state).await?;
    let raw_number = try_resolve_issue_number(&file, body.issue_number.as_deref(), &patterns)
        .ok_or_else(|| ApiError::Unprocessable {
            code: "unprocessable.issue_number_unresolved",
            message: "Could not determine issue number from ComicInfo or filename; \
                      re-submit with `issue_number` in the request body."
                .into(),
        })?;
    let target = IssueNumber::new(raw_number.clone());

    // Series's issue set is small (~25-300 rows) so listing + filtering is
    // cheaper than adding a new repo method right now.
    let candidates = issue_repo::list_by_series(&state.db, series.id).await?;
    let matched_issue = candidates
        .into_iter()
        .find(|i| IssueNumber::new(i.number.clone()).matches(&target))
        .ok_or_else(|| ApiError::Unprocessable {
            code: "unprocessable.issue_not_in_series",
            message: format!(
                "Series {} (cv_volume_id={}) has no issue numbered {:?}",
                series.id, body.cv_volume_id, raw_number
            ),
        })?;

    let now = now_utc_primitive();
    let new_issue_id = Some(matched_issue.id);
    let patch = FileUpdate {
        issue_id: new_issue_id,
        size_bytes: file.size_bytes,
        mtime: file.mtime,
        last_scanned_at: now,
        match_method: MatchMethod::Manual.as_db_str().to_owned(),
        match_confidence: 1.0,
        status: FileStatus::Owned.as_db_str().to_owned(),
        cached_comicinfo_xml: file.cached_comicinfo_xml.clone(),
        cached_at: file.cached_at,
        is_present: file.is_present,
        last_seen_at: file.last_seen_at,
        matched_at: file_repo::next_matched_at(file.issue_id, new_issue_id, file.matched_at, now),
    };
    file_repo::update(&state.db, id, patch).await?;

    // Fires only when the helper actually inserted a new series, OR when
    // an existing series's siblings might now match — either way, this is
    // cheap when there's nothing to do.
    spawn_auto_rematch(&state, series.id, "match-from-cv");

    Ok(Json(fetch_enriched_by_id(&state.db, id).await?))
}

/// Loads enabled parsing_patterns once for callers that resolve issue
/// numbers across many files (e.g. the folder-match endpoint).
async fn load_enabled_patterns(state: &AppState) -> Result<Vec<ParsingPattern>, ApiError> {
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

/// Resolves the issue number for a manual CV match. Priority:
/// 1. Caller-provided override.
/// 2. The file's cached ComicInfo `<Number>` field.
/// 3. Filename parsing using the supplied patterns.
///
/// `None` when no source yields a value. Callers map that to whatever
/// behaviour suits them (422 for single-file, skip-with-reason for the
/// folder endpoint).
fn try_resolve_issue_number(
    file: &FileRow,
    override_value: Option<&str>,
    patterns: &[ParsingPattern],
) -> Option<String> {
    if let Some(s) = override_value.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(s.to_owned());
    }

    if let Some(xml) = file.cached_comicinfo_xml.as_deref() {
        if let Ok(info) = longbox_core::ComicInfo::parse(xml.as_bytes()) {
            if let Some(n) = info.number.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                return Some(n.to_owned());
            }
        }
    }

    let basename = std::path::Path::new(&file.path_relative)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&file.path_relative);
    longbox_core::parse_filename(basename, patterns).map(|p| p.number)
}

#[derive(Debug, Deserialize)]
struct MatchFolderBody {
    /// Directory relative to the library root, no leading or trailing
    /// slash. Match-folder operates on files whose `path_relative` starts
    /// with `{directory}/`.
    directory: String,
    cv_volume_id: i64,
}

#[derive(Debug, Serialize)]
struct MatchFolderResponse {
    matched_count: u32,
    skipped: Vec<SkippedFile>,
}

#[derive(Debug, Serialize)]
struct SkippedFile {
    path: String,
    reason: SkipReason,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SkipReason {
    /// Could not extract an issue number from ComicInfo or the filename.
    IssueNumberUnresolved,
    /// Issue number resolved, but the chosen series doesn't contain it.
    IssueNotInSeries,
}

/// POST /api/files/match-folder-from-cv
///
/// Bulk-match every unmatched / needs-review file under `directory` to
/// the same CV volume. Adds the series to the watchlist if not already
/// present (one CV fetch for the whole batch). Skips files whose issue
/// number can't be resolved or isn't in the series, surfacing both in
/// the response so the UI can prompt the user for overrides.
async fn match_folder_from_cv(
    State(state): State<AppState>,
    Json(body): Json<MatchFolderBody>,
) -> Result<Json<MatchFolderResponse>, ApiError> {
    if body.cv_volume_id <= 0 {
        return Err(ApiError::BadRequest {
            message: "cv_volume_id must be > 0".into(),
        });
    }
    let directory = body.directory.trim().trim_matches('/').to_owned();
    if directory.is_empty() {
        return Err(ApiError::BadRequest {
            message: "directory must be a non-empty path relative to the library root".into(),
        });
    }

    let (series, _was_new) = add_or_get_from_cv(&state, body.cv_volume_id).await?;
    let candidates = issue_repo::list_by_series(&state.db, series.id).await?;

    // Substring-equality prefix match avoids the LIKE-wildcard escaping
    // gymnastics that a `LIKE 'dir/%'` query would require. Treats
    // `_` / `%` in folder names as literals.
    //
    // The `instr(...) = 0` clause restricts results to files immediately
    // in `directory` — no further descent. This keeps the backend's
    // file set aligned with the frontend's by-immediate-parent grouping
    // (so `directory: "Saga"` does not pick up `Saga/Annual/Annual 1.cbz`).
    let prefix = format!("{directory}/");
    let prefix_len = prefix.len() as i64;
    let library_root_id = state.library_root_id;
    let rows = sqlx::query!(
        r#"SELECT
             id AS "id!: i64",
             issue_id,
             library_root_id AS "library_root_id!: i64",
             path_relative,
             size_bytes AS "size_bytes!: i64",
             mtime AS "mtime!: time::PrimitiveDateTime",
             last_scanned_at AS "last_scanned_at!: time::PrimitiveDateTime",
             match_method, match_confidence, status,
             cached_comicinfo_xml,
             cached_at AS "cached_at: time::PrimitiveDateTime",
             is_present AS "is_present!: bool",
             last_seen_at AS "last_seen_at!: time::PrimitiveDateTime",
             matched_at AS "matched_at: time::PrimitiveDateTime"
           FROM files
           WHERE library_root_id = ?1
             AND status IN ('unmatched', 'needs_review')
             AND is_present = 1
             AND substr(path_relative, 1, ?2) = ?3
             AND instr(substr(path_relative, ?2 + 1), '/') = 0
           ORDER BY path_relative"#,
        library_root_id,
        prefix_len,
        prefix,
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("folder-match query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;

    let patterns = load_enabled_patterns(&state).await?;
    let now = now_utc_primitive();
    let mut matched_count: u32 = 0;
    let mut skipped: Vec<SkippedFile> = Vec::new();

    for r in rows {
        let file = FileRow {
            id: r.id,
            issue_id: r.issue_id,
            library_root_id: r.library_root_id,
            path_relative: r.path_relative,
            size_bytes: r.size_bytes,
            mtime: r.mtime,
            last_scanned_at: r.last_scanned_at,
            match_method: r.match_method,
            match_confidence: r.match_confidence,
            status: r.status,
            cached_comicinfo_xml: r.cached_comicinfo_xml,
            cached_at: r.cached_at,
            is_present: r.is_present,
            last_seen_at: r.last_seen_at,
            matched_at: r.matched_at,
        };

        let Some(raw_number) = try_resolve_issue_number(&file, None, &patterns) else {
            skipped.push(SkippedFile {
                path: file.path_relative,
                reason: SkipReason::IssueNumberUnresolved,
            });
            continue;
        };
        let target = IssueNumber::new(raw_number);
        let Some(issue) = candidates
            .iter()
            .find(|i| IssueNumber::new(i.number.clone()).matches(&target))
        else {
            skipped.push(SkippedFile {
                path: file.path_relative,
                reason: SkipReason::IssueNotInSeries,
            });
            continue;
        };

        let new_issue_id = Some(issue.id);
        let patch = FileUpdate {
            issue_id: new_issue_id,
            size_bytes: file.size_bytes,
            mtime: file.mtime,
            last_scanned_at: now,
            match_method: MatchMethod::Manual.as_db_str().to_owned(),
            match_confidence: 1.0,
            status: FileStatus::Owned.as_db_str().to_owned(),
            cached_comicinfo_xml: file.cached_comicinfo_xml.clone(),
            cached_at: file.cached_at,
            is_present: file.is_present,
            last_seen_at: file.last_seen_at,
            matched_at: file_repo::next_matched_at(
                file.issue_id,
                new_issue_id,
                file.matched_at,
                now,
            ),
        };
        file_repo::update(&state.db, file.id, patch).await?;
        matched_count += 1;
    }

    // Even when matched_count is 0, fire the rematch: a newly-added
    // series's sibling files in other folders may now match via the
    // normal scanner pipeline (CV URL / ComicInfo etc.).
    spawn_auto_rematch(&state, series.id, "match-folder-from-cv");

    Ok(Json(MatchFolderResponse {
        matched_count,
        skipped,
    }))
}
