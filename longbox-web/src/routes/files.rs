use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_core::{FileStatus, MatchMethod};
use longbox_db::{file_repo, issue_repo, FileRow, FileUpdate};
use serde::Deserialize;
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

async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<FileRow>>, ApiError> {
    let library_root_id = params
        .library_root_id
        .unwrap_or(state.library_root_id);
    let rows = match params.status.as_deref() {
        None | Some("all") => file_repo::list_by_library_root(&state.db, library_root_id).await?,
        Some(s) => {
            // Validate against the enum.
            if FileStatus::from_db_str(s).is_none() {
                return Err(ApiError::BadRequest {
                    message: format!("unknown status: {s:?}"),
                });
            }
            file_repo::list_by_status(&state.db, library_root_id, s).await?
        }
    };
    Ok(Json(rows))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<FileRow>, ApiError> {
    file_repo::find_by_id(&state.db, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound {
            resource: "file",
            id: id.to_string(),
        })
}

/// PATCH body shapes:
/// - `{ "issue_id": 42 }` — manual rematch: set issue, mark owned/manual.
/// - `{ "status": "ignored" }` — flag as not-a-comic.
/// - `{ "status": null }` — clear ignore: revert to needs_review / unmatched.
///
/// Other combinations are rejected as bad request.
#[derive(Debug, Deserialize)]
struct PatchBody {
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    issue_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    status: Option<Option<String>>,
}

/// `Option<Option<T>>` distinguishes "field absent" (`None`) from "field
/// present with null value" (`Some(None)`) from "field present with value"
/// (`Some(Some(v))`).
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
) -> Result<Json<FileRow>, ApiError> {
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
        // Manual rematch.
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
        // Mark ignored.
        (None, Some(Some(s))) if s == FileStatus::Ignored.as_db_str() => {
            patch.issue_id = None;
            patch.match_method = MatchMethod::Ignored.as_db_str().to_owned();
            patch.match_confidence = 0.0;
            patch.status = FileStatus::Ignored.as_db_str().to_owned();
        }
        // Clear ignored: revert. We can't re-run the matcher here without
        // candidate fetching; flip to needs_review (or unmatched if no
        // issue_id) and let the next scan re-classify properly.
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
        // Anything else.
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

    let updated = file_repo::update(&state.db, id, patch).await?;
    Ok(Json(updated))
}

fn now_utc_primitive() -> time::PrimitiveDateTime {
    let n = OffsetDateTime::now_utc();
    time::PrimitiveDateTime::new(n.date(), n.time())
}
