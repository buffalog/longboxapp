//! Phase B post-process surface.
//!
//! - `GET /api/postprocess/pending` returns `{ count, items }` from the
//!   pending-intervention cache; powers the dashboard counter tile and
//!   the `/files/pending-intervention` list view in one round-trip.
//! - `POST /api/postprocess/trigger` runs an on-demand drain of the
//!   watch folder via `longbox_postprocess::sweep_now` and returns a
//!   per-outcome tally so the Settings page can render a result toast.
//! - `POST /api/postprocess/import-anyway` files a page-count hold into
//!   the issue it was matched to, or an ambiguous hold into the candidate
//!   the user picked, once they have looked at it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_postprocess::processor::Outcome;
use longbox_postprocess::{ImportAnyway, PendingIntervention, SweepSummary};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/postprocess/pending", get(handler))
        .route("/postprocess/trigger", post(trigger))
        .route("/postprocess/import-anyway", post(import_anyway))
}

#[derive(Debug, Serialize)]
struct PendingResponse {
    count: usize,
    items: Vec<PendingIntervention>,
}

async fn handler(State(state): State<AppState>) -> Result<Json<PendingResponse>, ApiError> {
    let items = state.pending_cache.snapshot();
    Ok(Json(PendingResponse {
        count: items.len(),
        items,
    }))
}

/// Fire an on-demand Phase B sweep over the configured watch folder.
/// Returns the per-outcome tally so the UI can render
/// `processed N · conflicts M · unsorted K` in a toast without
/// needing to poll the pending-intervention cache for the count.
///
/// Refuses (400) when `DOWNLOAD_WATCH_PATH` is unset — there's nothing
/// to sweep. Watch-folder-missing maps to 503 because the surface IS
/// configured, just inaccessible at request time (mount lost, perms
/// flipped); distinct from "not configured" so the UI can phrase the
/// error correctly.
async fn trigger(State(state): State<AppState>) -> Result<Json<SweepSummary>, ApiError> {
    let Some(watch_path) = state.config.download_watch_path.as_deref() else {
        return Err(ApiError::BadRequest {
            message: "DOWNLOAD_WATCH_PATH is not configured; no watch folder to sweep.".into(),
        });
    };
    let library_root = Path::new(&state.config.library_root_path);
    let watch = Path::new(watch_path);
    let summary = longbox_postprocess::sweep_now(
        library_root,
        state.library_root_id,
        watch,
        state.db.clone(),
        Arc::clone(&state.pending_cache),
    )
    .await
    .map_err(|e| match e {
        longbox_postprocess::PostprocessError::WatchPathUnreadable { ref path, .. } => {
            ApiError::ServiceUnavailable {
                code: "watch_path_unreadable",
                message: format!(
                    "watch folder {} is configured but unreadable: {e}",
                    path.display()
                ),
            }
        }
        other => ApiError::Internal {
            message: format!("postprocess sweep failed: {other}"),
            source: anyhow::anyhow!(other),
        },
    })?;
    tracing::info!(
        target: "longbox_web",
        processed = summary.processed,
        skipped = summary.skipped,
        conflicts = summary.conflicts,
        failed = summary.failed,
        "postprocess.trigger_completed"
    );
    Ok(Json(summary))
}

#[derive(Debug, Deserialize)]
struct ImportAnywayBody {
    /// A `source_path` from `GET /api/postprocess/pending`.
    source_path: PathBuf,
    /// For an ambiguous hold: the candidate issue to file it as.
    #[serde(default)]
    issue_id: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ImportAnywayResponse {
    target: PathBuf,
    issue_id: i64,
}

/// File a page-count hold into the issue it was matched to, or an
/// ambiguous hold into the candidate `issue_id` the user picked. 404 when
/// the path is neither kind of hold in the pending list (nothing else is
/// eligible, and no other path is touched); 400 when an ambiguous hold's
/// `issue_id` is missing or not a candidate; 409 when the import itself
/// didn't land, e.g. the library already has that issue.
async fn import_anyway(
    State(state): State<AppState>,
    Json(body): Json<ImportAnywayBody>,
) -> Result<Json<ImportAnywayResponse>, ApiError> {
    let Some(watch_path) = state.config.download_watch_path.as_deref() else {
        return Err(ApiError::BadRequest {
            message: "DOWNLOAD_WATCH_PATH is not configured; no watch folder to import from."
                .into(),
        });
    };
    let result = longbox_postprocess::import_anyway(
        Path::new(&state.config.library_root_path),
        state.library_root_id,
        Path::new(watch_path),
        &state.db,
        &state.pending_cache,
        &body.source_path,
        body.issue_id,
    )
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("import anyway failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    match result {
        ImportAnyway::Ran(Outcome::Imported {
            target, issue_id, ..
        }) => Ok(Json(ImportAnywayResponse { target, issue_id })),
        ImportAnyway::NotHeld => Err(ApiError::NotFound {
            resource: "held file",
            id: body.source_path.display().to_string(),
        }),
        ImportAnyway::NotACandidate => Err(ApiError::BadRequest {
            message: "Pick one of the hold's candidate issues (issue_id).".into(),
        }),
        ImportAnyway::Ran(other) => Err(ApiError::Conflict {
            code: "conflict.import_anyway",
            message: match other {
                Outcome::Conflict { target, .. } => format!(
                    "The library already has this issue at {}; the held copy was removed.",
                    target.display()
                ),
                Outcome::Failed { reason, .. } => format!("The import failed: {reason:?}"),
                other => format!("The file was not imported: {other:?}"),
            },
            details: serde_json::Value::Null,
        }),
    }
}
