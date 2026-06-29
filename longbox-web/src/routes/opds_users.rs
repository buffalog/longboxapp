//! OPDS user account admin API (`/api/opds/users`).
//!
//! Admin-only CRUD over the `opds_users` table — no self-registration, no
//! email, no password reset. A password is set at creation; to change it,
//! delete and recreate. Unauthenticated like the rest of the admin app
//! (mounted on the main app port, never on the OPDS-only listener).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{opds_users_repo, OpdsUserRow};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// Minimum password length accepted at creation.
const MIN_PASSWORD_LEN: usize = 8;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/opds/users", get(list).post(create))
        .route("/opds/users/:id/enable", post(enable))
        .route("/opds/users/:id/disable", post(disable))
        .route("/opds/users/:id", axum::routing::delete(delete))
}

#[derive(Debug, Deserialize)]
struct CreateUser {
    username: String,
    password: String,
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<OpdsUserRow>>, ApiError> {
    Ok(Json(opds_users_repo::list(&state.db).await?))
}

/// Create an enabled account. Validates a non-empty username and a password of
/// at least [`MIN_PASSWORD_LEN`] chars; a duplicate username surfaces as 409
/// via the DB unique violation. Returns the created row (201).
async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateUser>,
) -> Result<(StatusCode, Json<OpdsUserRow>), ApiError> {
    let username = body.username.trim();
    if username.is_empty() {
        return Err(ApiError::BadRequest {
            message: "Username is required.".into(),
        });
    }
    if body.password.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::BadRequest {
            message: format!("Password must be at least {MIN_PASSWORD_LEN} characters."),
        });
    }
    let hash = longbox_opds::hash_password(&body.password).map_err(|err| ApiError::Internal {
        message: "failed to hash OPDS password".to_owned(),
        source: anyhow::Error::new(err),
    })?;
    let row = opds_users_repo::create(&state.db, username, &hash).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn enable(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    opds_users_repo::set_enabled(&state.db, id, true)
        .await
        .map_err(|e| not_found_to_user(e, id))?;
    Ok(Json(serde_json::json!({ "id": id, "enabled": true })))
}

async fn disable(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    opds_users_repo::set_enabled(&state.db, id, false)
        .await
        .map_err(|e| not_found_to_user(e, id))?;
    Ok(Json(serde_json::json!({ "id": id, "enabled": false })))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    opds_users_repo::delete(&state.db, id)
        .await
        .map_err(|e| not_found_to_user(e, id))?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

/// Map the repo's generic `NotFound` to a clean `opds_user` 404; pass anything
/// else through unchanged.
fn not_found_to_user(err: longbox_db::DbError, id: i64) -> ApiError {
    match err {
        longbox_db::DbError::NotFound => ApiError::NotFound {
            resource: "opds_user",
            id: id.to_string(),
        },
        other => other.into(),
    }
}
