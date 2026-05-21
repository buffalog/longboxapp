//! The needs-attention surface — pull-side failures.
//!
//! `GET /api/needs-attention/pull-failures` lists issues whose most
//! recent pull attempt failed; `POST /api/needs-attention/retry`
//! un-parks an issue (clears its failed attempts) and nudges a sweep.
//! Phase B's intervention failures keep their own endpoint
//! (`/api/postprocess/pending`) — the `/needs-attention` page reads
//! both and renders them as two sections.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{pull_attempt_repo, FailedPull};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/needs-attention/pull-failures", get(pull_failures))
        .route("/needs-attention/retry", post(retry))
}

/// A pull failure plus its category. `category` is derived from
/// `release_id`: a failed attempt that never carried a release is a
/// submission failure; one that did is a grab failure.
#[derive(Debug, Serialize)]
struct PullFailureRow {
    #[serde(flatten)]
    failure: FailedPull,
    category: &'static str,
}

#[derive(Debug, Deserialize)]
struct RetryBody {
    series_id: i64,
    issue_id: i64,
}

async fn pull_failures(
    State(state): State<AppState>,
) -> Result<Json<Vec<PullFailureRow>>, ApiError> {
    let rows = pull_attempt_repo::list_failed(&state.db)
        .await?
        .into_iter()
        .map(|failure| {
            let category = if failure.release_id.is_none() {
                "submission_failed"
            } else {
                "grab_failed"
            };
            PullFailureRow { failure, category }
        })
        .collect();
    Ok(Json(rows))
}

/// Un-park an issue: clear its `failed` pull attempts so the next sweep
/// retries it, then nudge an immediate sweep. Idempotent — a stale UI
/// retrying an already-cleared issue is a clean no-op.
async fn retry(
    State(state): State<AppState>,
    Json(body): Json<RetryBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cleared =
        pull_attempt_repo::clear_failed_for_issue(&state.db, body.series_id, body.issue_id).await?;
    // Best-effort — if a sweep is already running, the un-parked issue
    // is picked up by the next one.
    state.pull.request_sweep();
    Ok(Json(serde_json::json!({ "cleared": cleared })))
}
