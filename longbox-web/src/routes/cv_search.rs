use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_comicvine::SeriesSearchResult;
use longbox_db::{publisher_filter_repo, series_repo};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cv/search", get(handler))
        .route("/cv/rate-limit", get(rate_limit))
}

/// Read-only view of the ComicVine lanes: each lane's sliding-window
/// call count against its hourly budget, plus whether background work is
/// yielding to the user or both lanes are paused on a 429. Drives the
/// nav-bar chip. `window_started_at_unix` is a unix-second timestamp.
#[derive(Debug, Serialize)]
struct RateLimitResponse {
    /// Includes the delta refresh, which spends the interactive budget.
    interactive: LaneCount,
    background: LaneCount,
    yielding_seconds: u64,
    paused_seconds: u64,
}

#[derive(Debug, Serialize)]
struct LaneCount {
    count: u32,
    limit_per_hour: u32,
    window_started_at_unix: i64,
}

impl From<longbox_comicvine::CvRateLimitSnapshot> for LaneCount {
    fn from(s: longbox_comicvine::CvRateLimitSnapshot) -> Self {
        Self {
            count: s.count,
            limit_per_hour: s.limit_per_hour,
            window_started_at_unix: s.window_started_at_unix,
        }
    }
}

async fn rate_limit(State(state): State<AppState>) -> Json<RateLimitResponse> {
    let snap = state.cv.coordinator().snapshot();
    // Round up so a pause with 0.4s left still reads "paused".
    let secs = |d: std::time::Duration| d.as_secs() + u64::from(d.subsec_nanos() > 0);
    Json(RateLimitResponse {
        interactive: snap.interactive.into(),
        background: snap.background.into(),
        yielding_seconds: secs(snap.yielding_for),
        paused_seconds: secs(snap.paused_for),
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

/// A CV search result plus a local-library annotation. `SeriesSearchResult`
/// is a pure CV projection (no DB awareness), so "is this already tracked?"
/// is layered on here in the web tier rather than on the domain struct.
#[derive(Debug, Serialize)]
struct SearchResultOut {
    #[serde(flatten)]
    result: SeriesSearchResult,
    /// Local series id when this CV volume is already tracked; `None`
    /// otherwise. Drives the "In Library" badge + "View Series" link.
    in_library_series_id: Option<i64>,
}

#[derive(Debug, Serialize)]
struct CvSearchResponse {
    results: Vec<SearchResultOut>,
    /// Results hidden because their publisher is on the blocklist. Always 0
    /// when `show_filtered=true`.
    filtered_publisher: u32,
    /// Results hidden because the CV volume is already in the library. Always
    /// 0 when `show_filtered=true`. A result that is *both* owned and
    /// publisher-blocked counts only here (in-library precedence), so the two
    /// counts sum to total hidden with no double-count.
    filtered_in_library: u32,
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

    // Pure local lookup (no extra CV calls): which CV volume ids are already
    // tracked, and their local series ids.
    let owned = series_repo::owned_cv_ids(&state.db).await?;

    if params.show_filtered {
        // Reveal everything, annotating owned results so the frontend can
        // badge them and swap Add for View Series.
        let results = all
            .into_iter()
            .map(|v| SearchResultOut {
                in_library_series_id: owned.get(&v.cv_id).copied(),
                result: v,
            })
            .collect();
        return Ok(Json(CvSearchResponse {
            results,
            filtered_publisher: 0,
            filtered_in_library: 0,
        }));
    }

    // Apply both hidden buckets. Names are stored COLLATE NOCASE in the DB
    // and lowercased by the repo helper; we lowercase CV's publisher field
    // too before comparing.
    let blocked = publisher_filter_repo::blocked_names_lower(&state.db).await?;

    let mut filtered_publisher = 0u32;
    let mut filtered_in_library = 0u32;
    let mut results = Vec::new();
    for v in all {
        // In-library precedence: owned results are hidden and counted here,
        // never double-counted against the publisher bucket.
        if owned.contains_key(&v.cv_id) {
            filtered_in_library = filtered_in_library.saturating_add(1);
            continue;
        }
        let is_blocked = match v.publisher.as_deref() {
            // Normalise the CV side to match `blocked_names_lower`
            // (trim + lowercase) so whitespace/casing can't defeat a block.
            Some(p) => {
                let norm = p.trim().to_lowercase();
                blocked.iter().any(|b| b == &norm)
            }
            None => false,
        };
        if is_blocked {
            filtered_publisher = filtered_publisher.saturating_add(1);
            continue;
        }
        results.push(SearchResultOut {
            result: v,
            in_library_series_id: None,
        });
    }

    Ok(Json(CvSearchResponse {
        results,
        filtered_publisher,
        filtered_in_library,
    }))
}
