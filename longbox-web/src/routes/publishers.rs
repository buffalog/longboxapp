use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{
    publisher_filter_repo, NewPublisherFilter, PublisherFilterMode, PublisherFilterRow,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/publishers/filters", get(list).post(create))
        .route("/publishers/filters/:id", axum::routing::delete(remove))
        .route("/publishers/filters/reset-defaults", post(reset_defaults))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    publisher_name: String,
    /// Optional; defaults to `block`. UI never sends `allow` today, but
    /// the column accepts both at the DB layer for forward-compat.
    mode: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResetResponse {
    inserted: u64,
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<PublisherFilterRow>>, ApiError> {
    Ok(Json(publisher_filter_repo::list_all(&state.db).await?))
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> Result<Json<PublisherFilterRow>, ApiError> {
    let name = body.publisher_name.trim().to_owned();
    if name.is_empty() {
        return Err(ApiError::BadRequest {
            message: "publisher_name must not be empty".into(),
        });
    }
    let mode = match body.mode.as_deref().unwrap_or("block") {
        "block" => PublisherFilterMode::Block,
        "allow" => PublisherFilterMode::Allow,
        other => {
            return Err(ApiError::BadRequest {
                message: format!("mode must be 'block' or 'allow', got {other:?}"),
            });
        }
    };
    match publisher_filter_repo::insert(
        &state.db,
        NewPublisherFilter {
            publisher_name: name.clone(),
            mode,
        },
    )
    .await
    {
        Ok(row) => Ok(Json(row)),
        Err(longbox_db::DbError::UniqueViolation { field: "publisher_name" }) => {
            Err(ApiError::Conflict {
                code: "conflict.publisher_filter_exists",
                message: format!("A filter for {name:?} already exists (case-insensitive)."),
            })
        }
        Err(e) => Err(ApiError::from(e)),
    }
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    publisher_filter_repo::delete(&state.db, id)
        .await
        .map_err(|e| match e {
            longbox_db::DbError::NotFound => ApiError::NotFound {
                resource: "publisher_filter",
                id: id.to_string(),
            },
            other => ApiError::from(other),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_defaults(State(state): State<AppState>) -> Result<Json<ResetResponse>, ApiError> {
    let inserted = publisher_filter_repo::reset_to_defaults(&state.db).await?;
    Ok(Json(ResetResponse { inserted }))
}
