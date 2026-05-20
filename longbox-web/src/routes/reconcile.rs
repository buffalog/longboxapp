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

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use longbox_db::{discovered_folders_repo, series_repo, DiscoveredFolderRow, PhantomSeries};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::routes::series::{add_or_get_from_cv, delete_series, spawn_auto_rematch};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/reconcile/phantoms", get(phantoms))
        .route("/reconcile/untracked", get(untracked))
        .route("/reconcile/add", post(add))
        .route("/reconcile/dismiss", post(dismiss))
        .route("/reconcile/phantom/:series_id", delete(delete_phantom))
        .route("/reconcile/phantom/:series_id/keep", post(keep_phantom))
        .route("/reconcile/phantoms/bulk", post(bulk_delete_phantoms))
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

/// Non-dismissed discovered folders — the untracked working set.
async fn untracked(
    State(state): State<AppState>,
) -> Result<Json<Vec<DiscoveredFolderRow>>, ApiError> {
    Ok(Json(discovered_folders_repo::list(&state.db).await?))
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
    discovered_folders_repo::dismiss(&state.db, std::slice::from_ref(&folder.folder_name)).await?;
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

/// "Keep" a transition phantom: reset `last_matched_count` to 0 so it
/// demotes from the "recently lost files" surface to the steady-state
/// list. The user has reviewed the lost-files signal and decided to
/// keep the catalog entry. 404 for an unknown series — the explicit
/// existence check gives a clean `series` 404 rather than the generic
/// `row` one `update_last_matched_count`'s own `NotFound` would map to.
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
    series_repo::update_last_matched_count(&state.db, series_id, 0).await?;
    Ok(Json(serde_json::json!({ "kept": series_id })))
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
