//! Phase B post-process surface. Currently exposes the
//! pending-intervention cache through a single read endpoint.
//!
//! `GET /api/postprocess/pending` returns `{ count, items }` so the
//! dashboard counter tile and the `/files/pending-intervention` list
//! view share a single backend call without the dashboard having to
//! pull the full vector for what is normally a zero-length read.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use longbox_postprocess::PendingIntervention;
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/postprocess/pending", get(handler))
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
