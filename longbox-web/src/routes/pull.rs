//! Pull engine — the manual "Check now" sweep trigger.
//!
//! The scheduled sweep and the engine itself live in `longbox-pull`;
//! this route only nudges the running scheduler. The pull-list
//! management UI is Phase A.8 Step 7.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/pull/check", post(check_now))
}

/// Request an immediate pull sweep. `202 Accepted` when the request is
/// taken; `409 Conflict` when a sweep is already running — the engine
/// runs sweeps strictly one at a time.
async fn check_now(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    if state.pull.request_sweep() {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError::Conflict {
            code: "conflict.pull_running",
            message: "A pull sweep is already running.".into(),
        })
    }
}
