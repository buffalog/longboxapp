use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_db::creator_repo::{self, CreatorDetail, CreatorIssueRow, CreatorSearchRow};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/creators/search", get(search_handler))
        .route("/creators/:id", get(detail_handler))
        .route("/creators/:id/issues", get(issues_handler))
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: String,
}

async fn search_handler(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<CreatorSearchRow>>, ApiError> {
    let q = params.q.trim();
    if q.chars().count() < 2 {
        return Err(ApiError::BadRequest {
            message: "query parameter `q` must be at least 2 characters".into(),
        });
    }
    let rows = creator_repo::search_creators(&state.db, q).await?;
    Ok(Json(rows))
}

async fn detail_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<CreatorDetail>, ApiError> {
    match creator_repo::creator_detail(&state.db, id).await? {
        Some(detail) => Ok(Json(detail)),
        None => Err(ApiError::NotFound {
            resource: "creator",
            id: id.to_string(),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct IssuesParams {
    role: Option<String>,
    series_id: Option<i64>,
    #[serde(default = "default_page")]
    page: i64,
}

fn default_page() -> i64 {
    1
}

async fn issues_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<IssuesParams>,
) -> Result<Json<Vec<CreatorIssueRow>>, ApiError> {
    let rows = creator_repo::creator_issues(
        &state.db,
        id,
        params.role.as_deref(),
        params.series_id,
        params.page,
    )
    .await?;
    Ok(Json(rows))
}
