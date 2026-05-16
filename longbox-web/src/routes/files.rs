use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_core::{FileStatus, MatchMethod};
use longbox_db::{file_repo, issue_repo, FileRow, FileUpdate};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/files", get(list))
        .route("/files/:id", get(detail).patch(update))
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

    file_repo::update(&state.db, id, patch).await?;
    Ok(Json(fetch_enriched_by_id(&state.db, id).await?))
}

fn now_utc_primitive() -> time::PrimitiveDateTime {
    let n = OffsetDateTime::now_utc();
    time::PrimitiveDateTime::new(n.date(), n.time())
}
