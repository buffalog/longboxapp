use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_comicvine::SeriesSearchResult;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/cv/search", get(handler))
}

#[derive(Debug, Deserialize)]
struct Params {
    q: String,
}

async fn handler(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> Result<Json<Vec<SeriesSearchResult>>, ApiError> {
    let query = params.q.trim();
    if query.is_empty() {
        return Err(ApiError::BadRequest {
            message: "query parameter `q` must be non-empty".into(),
        });
    }
    let results = state.cv.search_volumes(query).await?;
    Ok(Json(results))
}
