use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_comicvine::SeriesSearchResult;
use longbox_db::publisher_filter_repo;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cv/search", get(handler))
        .route("/cv/rate-limit", get(rate_limit))
}

/// Read-only view of the ComicVine client's sliding-window call
/// counter. Drives the dashboard / Settings chip that surfaces "42/100
/// this hour" so the user can see when they're nearing the quota.
/// `window_started_at` is a unix-second timestamp; the frontend
/// formats it relative.
#[derive(Debug, Serialize)]
struct RateLimitResponse {
    count: u32,
    limit_per_hour: u32,
    window_started_at_unix: i64,
}

async fn rate_limit(State(state): State<AppState>) -> Json<RateLimitResponse> {
    let snap = state.cv.rate_limit_snapshot();
    Json(RateLimitResponse {
        count: snap.count,
        limit_per_hour: snap.limit_per_hour,
        window_started_at_unix: snap.window_started_at_unix,
    })
}

#[derive(Debug, Deserialize)]
struct Params {
    q: String,
    /// When `true`, skip the publisher blocklist and return everything.
    /// Default unset = filter applied.
    #[serde(default)]
    show_filtered: bool,
}

#[derive(Debug, Serialize)]
struct CvSearchResponse {
    results: Vec<SeriesSearchResult>,
    /// How many results the blocklist removed for this call. Always 0
    /// when `show_filtered=true`. Frontend uses this to show "N more
    /// filtered — show them" UX.
    filtered_count: u32,
}

async fn handler(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> Result<Json<CvSearchResponse>, ApiError> {
    let query = params.q.trim();
    if query.is_empty() {
        return Err(ApiError::BadRequest {
            message: "query parameter `q` must be non-empty".into(),
        });
    }
    let all = state.cv.search_volumes(query).await?;

    if params.show_filtered {
        return Ok(Json(CvSearchResponse {
            results: all,
            filtered_count: 0,
        }));
    }

    // Apply the publisher blocklist. Names are stored COLLATE NOCASE in
    // the DB and lowercased by the repo helper; we lowercase CV's
    // publisher field too before comparing.
    let blocked = publisher_filter_repo::blocked_names_lower(&state.db).await?;
    if blocked.is_empty() {
        return Ok(Json(CvSearchResponse {
            results: all,
            filtered_count: 0,
        }));
    }

    let before = all.len();
    let results: Vec<SeriesSearchResult> = all
        .into_iter()
        .filter(|v| match v.publisher.as_deref() {
            Some(p) => !blocked.iter().any(|b| b == &p.to_lowercase()),
            None => true,
        })
        .collect();
    let filtered_count = u32::try_from(before - results.len()).unwrap_or(u32::MAX);

    Ok(Json(CvSearchResponse {
        results,
        filtered_count,
    }))
}
