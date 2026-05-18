use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/settings", get(handler))
}

/// Phase A settings are read-only and baked at server start from env
/// vars. This endpoint just reflects them back so the Settings page can
/// show the user what their container is actually pointed at.
///
/// `comicvine_api_key_configured` is structurally always `true` today —
/// the server refuses to start without a non-empty key. The field is
/// retained as a forward-compatible boolean so the wire contract doesn't
/// have to break later if an in-app key-setting flow is added.
#[derive(Debug, Serialize)]
struct SettingsResponse {
    library_root_path: String,
    database_url: String,
    bind_address: String,
    log_level: String,
    match_threshold: f64,
    comicvine_api_key_configured: bool,
    /// Phase B's `DOWNLOAD_WATCH_PATH`. `None` = Phase B not
    /// configured for this deployment; otherwise the configured path
    /// (whether currently readable or not).
    download_watch_path: Option<String>,
    version: &'static str,
}

async fn handler(State(state): State<AppState>) -> Json<SettingsResponse> {
    Json(SettingsResponse {
        library_root_path: state.config.library_root_path.clone(),
        database_url: state.config.database_url.clone(),
        bind_address: state.config.bind_addr.clone(),
        log_level: state.config.log_level.clone(),
        match_threshold: state.config.match_threshold,
        // Boot fails without it, so this is always `true` today. Boolean
        // shape kept for future-proofing.
        comicvine_api_key_configured: !state.config.comicvine_api_key.is_empty(),
        download_watch_path: state.config.download_watch_path.clone(),
        version: env!("CARGO_PKG_VERSION"),
    })
}
