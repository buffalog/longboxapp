//! OPDS access settings admin API (`/api/opds/settings`).
//!
//! Distinct from the OPDS *catalog* (`/opds/v1/*`, auth-gated): this is the
//! LongBox admin surface the Settings UI uses to configure the catalog's
//! credentials. It lives under `/api` and is unauthenticated like the rest
//! of the admin app — the OPDS-only auth decision does not extend here.
//!
//! The password hash is never returned (only `has_password`); the API
//! token IS returned so the user can copy it into a reader. The bearer
//! token is generated on the first save and regenerable on demand.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::settings_repo;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/opds/settings", get(get_settings).put(update_settings))
        .route("/opds/settings/regenerate-token", post(regenerate_token))
}

/// OPDS settings as the UI sees them. The password is represented only by
/// `has_password`; the hash never leaves the server.
#[derive(Debug, Serialize)]
struct OpdsSettings {
    enabled: bool,
    username: String,
    has_password: bool,
    api_token: String,
    base_url: String,
    /// Convenience: the catalog root URL to show as a copyable link.
    catalog_url: String,
}

#[derive(Debug, Deserialize)]
struct OpdsUpdate {
    /// Toggle the catalog on/off.
    enabled: Option<bool>,
    /// Basic-auth username.
    username: Option<String>,
    /// Plaintext password; hashed before storage. An empty/whitespace
    /// value is ignored (set-only — submitting the form without retyping
    /// the password leaves it unchanged).
    password: Option<String>,
}

async fn get_settings(State(state): State<AppState>) -> Result<Json<OpdsSettings>, ApiError> {
    Ok(Json(load(&state).await?))
}

/// Apply the provided fields, generate the API token on first save if it's
/// still empty, and return the resulting settings.
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

    if let Some(username) = body.username {
        settings_repo::set(
            &state.db,
            settings_repo::KEY_OPDS_USERNAME,
            username.trim(),
        )
        .await?;
    }

    if let Some(password) = body.password {
        if !password.trim().is_empty() {
            let hash = longbox_opds::hash_password(&password).map_err(|err| ApiError::Internal {
                message: "failed to hash OPDS password".to_owned(),
                source: anyhow::Error::new(err),
            })?;
            settings_repo::set(&state.db, settings_repo::KEY_OPDS_PASSWORD_HASH, &hash).await?;
        }
    }

    // First-save token generation.
    ensure_token(&state).await?;

    Ok(Json(load(&state).await?))
}

async fn regenerate_token(State(state): State<AppState>) -> Result<Json<OpdsSettings>, ApiError> {
    settings_repo::set(
        &state.db,
        settings_repo::KEY_OPDS_API_TOKEN,
        &longbox_opds::generate_api_token(),
    )
    .await?;
    Ok(Json(load(&state).await?))
}

/// Generate + store an API token only if none exists yet.
async fn ensure_token(state: &AppState) -> Result<(), ApiError> {
    let current = settings_repo::get(&state.db, settings_repo::KEY_OPDS_API_TOKEN).await?;
    if current.map(|t| t.trim().is_empty()).unwrap_or(true) {
        settings_repo::set(
            &state.db,
            settings_repo::KEY_OPDS_API_TOKEN,
            &longbox_opds::generate_api_token(),
        )
        .await?;
    }
    Ok(())
}

async fn load(state: &AppState) -> Result<OpdsSettings, ApiError> {
    let enabled: bool =
        settings_repo::get_or_default(&state.db, settings_repo::KEY_OPDS_ENABLED, false).await?;
    let username = settings_repo::get(&state.db, settings_repo::KEY_OPDS_USERNAME)
        .await?
        .unwrap_or_default();
    let has_password = settings_repo::get(&state.db, settings_repo::KEY_OPDS_PASSWORD_HASH)
        .await?
        .is_some_and(|h| !h.trim().is_empty());
    let api_token = settings_repo::get(&state.db, settings_repo::KEY_OPDS_API_TOKEN)
        .await?
        .unwrap_or_default();
    let base_url = state.config.opds_base_url.clone();
    let catalog_url = format!("{base_url}/opds/v1");

    Ok(OpdsSettings {
        enabled,
        username,
        has_password,
        api_token,
        base_url,
        catalog_url,
    })
}
