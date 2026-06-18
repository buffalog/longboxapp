//! Interactive Search — manual, ad-hoc NZB search for a specific issue.
//!
//! `POST /api/search/interactive` fans the (client-normalized) query out to
//! every configured indexer in parallel, scores each result against the
//! series title, and returns ALL results regardless of confidence plus
//! per-indexer errors — the user reviews and decides, the engine doesn't.
//! No `pull_attempt` is created here or on grab (see `interactive_search`
//! grab endpoint in a later commit).

use std::collections::HashMap;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use longbox_core::ParsingPattern;
use longbox_db::{
    downloader_config_repo, indexer_config_repo, issue_repo, parsing_pattern_repo, series_repo,
    DownloaderConfigRow, Pool,
};
use longbox_downloader::{connect, AnyDownloader, Downloader, DownloaderAuth, DownloaderConfig};
use longbox_newznab::{score_release_title, search_all_indexers, IndexerConfig, IndexerId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/search/interactive", post(interactive_search))
        .route("/search/interactive/grab", post(grab))
}

#[derive(Debug, Deserialize)]
struct SearchRequest {
    series_id: i64,
    issue_id: i64,
    query: String,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
    errors: Vec<IndexerErrorEntry>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    title: String,
    /// Human-readable indexer name (resolved from the indexer id).
    indexer: String,
    size_bytes: Option<i64>,
    age_days: Option<i64>,
    /// Series-title match score 0.0–1.0 (see `score_release_title`).
    confidence: f64,
    nzb_url: String,
}

#[derive(Debug, Serialize)]
struct IndexerErrorEntry {
    indexer: String,
    error: String,
}

async fn interactive_search(
    State(state): State<AppState>,
    Json(body): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    // Score against the series title (the query box is user-editable, so it
    // isn't a reliable scoring anchor). 404 if the series is unknown.
    let series = series_repo::find_by_id(&state.db, body.series_id)
        .await?
        .ok_or(ApiError::NotFound {
            resource: "series",
            id: body.series_id.to_string(),
        })?;

    // The issue's cover_date widens each indexer's maxage for back-catalog
    // issues (same fix the engine uses) so old NZBs aren't filtered out.
    let cover_date = issue_repo::find_by_id(&state.db, body.issue_id)
        .await?
        .and_then(|issue| issue.cover_date);

    let indexers = load_indexers(&state.db).await?;
    let patterns = load_patterns(&state.db).await?;
    let names: HashMap<i64, String> = indexers.iter().map(|i| (i.id.0, i.name.clone())).collect();

    let outcome = search_all_indexers(&indexers, body.query.trim(), cover_date.as_deref()).await;

    let today = time::OffsetDateTime::now_utc().date();
    let mut results: Vec<SearchResult> = outcome
        .results
        .into_iter()
        .map(|(indexer_id, release)| SearchResult {
            confidence: score_release_title(&release.title, &series.title, &patterns)
                .unwrap_or(0.0),
            age_days: release
                .published
                .map(|p| (today - p.date()).whole_days().max(0)),
            indexer: names.get(&indexer_id.0).cloned().unwrap_or_default(),
            size_bytes: release.size_bytes,
            title: release.title,
            nzb_url: release.nzb_url,
        })
        .collect();

    // Default order: confidence descending. The table is user-sortable
    // client-side; this just makes the first render sensible.
    results.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let errors = outcome
        .errors
        .into_iter()
        .map(|(id, err)| IndexerErrorEntry {
            indexer: names.get(&id.0).cloned().unwrap_or_default(),
            error: err.to_string(),
        })
        .collect();

    Ok(Json(SearchResponse { results, errors }))
}

// ---------- grab ----------

#[derive(Debug, Deserialize)]
struct GrabRequest {
    /// NZB URL from a search result. Round-tripped from the client per
    /// spec; never logged (newznab URLs embed the indexer API key).
    nzb_url: String,
    /// Human-readable name shown in the downloader queue.
    title: String,
}

#[derive(Debug, Serialize)]
struct GrabResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `POST /api/search/interactive/grab` — submit an NZB to the configured
/// downloader (SAB or NZBGet). No `pull_attempt`, no LongBox tracking — SAB
/// handles it and Phase B picks it up on completion. Always 200; the body's
/// `success` flag carries the outcome (matches the spec's contract).
async fn grab(
    State(state): State<AppState>,
    Json(body): Json<GrabRequest>,
) -> Result<Json<GrabResponse>, ApiError> {
    let Some(row) = downloader_config_repo::get(&state.db).await? else {
        return Ok(Json(GrabResponse {
            success: false,
            error: Some("No downloader is configured.".to_owned()),
        }));
    };

    let downloader = build_downloader(row);
    match downloader.submit(&body.nzb_url, &body.title).await {
        Ok(_handle) => Ok(Json(GrabResponse {
            success: true,
            error: None,
        })),
        Err(err) => {
            // Log the title only — never the nzb_url (carries the apikey).
            tracing::warn!(
                title = %body.title,
                error = %err,
                "interactive grab: downloader submit failed"
            );
            Ok(Json(GrabResponse {
                success: false,
                error: Some(err.to_string()),
            }))
        }
    }
}

/// Build the configured downloader client. Mirrors the pull engine's
/// private `build_downloader` (SAB → apikey auth, NZBGet → basic auth).
fn build_downloader(row: DownloaderConfigRow) -> AnyDownloader {
    let auth = match row.kind.as_str() {
        "nzbget" => DownloaderAuth::Basic {
            username: row.username.unwrap_or_default(),
            password: row.secret,
        },
        _ => DownloaderAuth::ApiKey(row.secret),
    };
    connect(&DownloaderConfig {
        base_url: row.base_url,
        auth,
        category: row.category,
    })
}

/// Map enabled indexer rows into the newznab config type. Mirrors the pull
/// engine's private `to_indexer_config` — `longbox-newznab` deliberately
/// doesn't depend on `longbox-db`, so the mapping lives at each boundary.
async fn load_indexers(db: &Pool) -> Result<Vec<IndexerConfig>, ApiError> {
    Ok(indexer_config_repo::list_enabled(db)
        .await?
        .into_iter()
        .map(|row| IndexerConfig {
            id: IndexerId(row.id),
            name: row.name,
            base_url: row.base_url,
            api_key: row.api_key,
            priority: i32::try_from(row.priority).unwrap_or(i32::MAX),
            maxage_days: row.maxage_days.max(0) as u32,
        })
        .collect())
}

/// Map enabled parsing-pattern rows into `longbox-core`'s type. Mirrors the
/// engine's `load_patterns` so release-title parsing matches the pull path.
async fn load_patterns(db: &Pool) -> Result<Vec<ParsingPattern>, ApiError> {
    Ok(parsing_pattern_repo::list_enabled(db)
        .await?
        .into_iter()
        .map(|r| ParsingPattern {
            id: r.id,
            name: r.name,
            pattern: r.pattern,
            priority: i32::try_from(r.priority).unwrap_or(i32::MAX),
            enabled: r.enabled,
        })
        .collect())
}
