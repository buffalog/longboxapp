//! OPDS global settings admin API (`/api/opds/settings`).
//!
//! Just the global `opds_enabled` toggle plus the info the Settings UI needs
//! to render a copyable catalog URL (the dedicated OPDS port and the optional
//! configured public base URL). Per-user accounts are managed separately under
//! `/api/opds/users` (see [`crate::routes::opds_users`]). Unauthenticated like
//! the rest of the admin app.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use longbox_db::settings_repo;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/opds/settings", get(get_settings).put(update_settings))
}

/// Global OPDS settings as the UI sees them.
#[derive(Debug, Serialize)]
struct OpdsSettings {
    enabled: bool,
    /// Dedicated OPDS listener port. The UI composes the catalog URL from
    /// the browser's host + this port when `base_url` is empty.
    opds_port: u16,
    /// Configured public base URL (`OPDS_BASE_URL`), or empty. When set, it's
    /// the authoritative catalog origin (e.g. behind a reverse proxy) and the
    /// UI shows `{base_url}/opds/v1` verbatim.
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct OpdsUpdate {
    /// Toggle the catalog on/off.
    enabled: Option<bool>,
}

async fn get_settings(State(state): State<AppState>) -> Result<Json<OpdsSettings>, ApiError> {
    Ok(Json(load(&state).await?))
}

async fn update_settings(
    State(state): State<AppState>,
    Json(body): Json<OpdsUpdate>,
) -> Result<Json<OpdsSettings>, ApiError> {
    if let Some(enabled) = body.enabled {
        settings_repo::set(
            &state.db,
            settings_repo::KEY_OPDS_ENABLED,
            if enabled { "true" } else { "false" },
        )
        .await?;
    }
    Ok(Json(load(&state).await?))
}

async fn load(state: &AppState) -> Result<OpdsSettings, ApiError> {
    let enabled: bool =
        settings_repo::get_or_default(&state.db, settings_repo::KEY_OPDS_ENABLED, false).await?;
    Ok(OpdsSettings {
        enabled,
        opds_port: state.config.opds_port,
        base_url: state.config.opds_base_url.clone(),
    })
}
