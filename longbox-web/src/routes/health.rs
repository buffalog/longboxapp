use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(handler))
}

async fn handler() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
