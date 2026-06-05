//! Phase B post-process surface.
//!
//! - `GET /api/postprocess/pending` returns `{ count, items }` from the
//!   pending-intervention cache; powers the dashboard counter tile and
//!   the `/files/pending-intervention` list view in one round-trip.
//! - `POST /api/postprocess/trigger` runs an on-demand drain of the
//!   watch folder via `longbox_postprocess::sweep_now` and returns a
//!   per-outcome tally so the Settings page can render a result toast.

use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_postprocess::{PendingIntervention, SweepSummary};
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/postprocess/pending", get(handler))
        .route("/postprocess/trigger", post(trigger))
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
