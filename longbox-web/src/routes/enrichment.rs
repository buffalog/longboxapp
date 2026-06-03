//! `/api/library/tidy/enrichment-summary` (GET),
//! `/api/library/tidy/enrichment-queue` (GET),
//! `/api/library/tidy/enrich-now` (POST).
//!
//! Aggregate + per-series surface for the Library Tidy enrichment
//! tab. `enrichment-summary` returns the counts the dashboard chip
//! reads. `enrichment-queue` is the disambiguation list — every
//! shallow series the worker landed on a review-required outcome,
//! ordered by impact so the user resolves the biggest libraries
//! first.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{series_repo, EnrichmentQueueRow};
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/library/tidy/enrichment-summary", get(summary))
        .route("/library/tidy/enrichment-queue", get(queue))
        .route("/library/tidy/enrich-now", post(enrich_now))
}

async fn queue(
    State(state): State<AppState>,
) -> Result<Json<Vec<EnrichmentQueueRow>>, ApiError> {
    Ok(Json(series_repo::list_enrichment_queue(&state.db).await?))
}

#[derive(Debug, Serialize)]
struct EnrichmentSummary {
    shallow_total: i64,
    awaiting_attempt: i64,
    cooldown_waiting: i64,
    is_running: bool,
    recent_outcomes: RecentOutcomeCounts,
}

#[derive(Debug, Serialize, Default)]
struct RecentOutcomeCounts {
    matched: i64,
    partial_merge: i64,
    no_results: i64,
    low_confidence: i64,
    multi_match: i64,
    year_mismatch: i64,
    count_mismatch: i64,
    collision_disabled: i64,
    cv_id_collision: i64,
    set_cv_id_race_lost: i64,
    error: i64,
}

async fn summary(State(state): State<AppState>) -> Result<Json<EnrichmentSummary>, ApiError> {
    // Three numbers + an outcome breakdown. All single SELECTs.
    let shallow_total = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM series WHERE cv_id IS NULL"#
    )
    .fetch_one(&state.db)
    .await
    .map_err(longbox_db::DbError::from)?;
    let awaiting_attempt = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM series
           WHERE cv_id IS NULL AND last_enrichment_attempt_at IS NULL"#
    )
    .fetch_one(&state.db)
    .await
    .map_err(longbox_db::DbError::from)?;
    let cooldown_waiting = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM series
           WHERE cv_id IS NULL AND last_enrichment_attempt_at IS NOT NULL"#
    )
    .fetch_one(&state.db)
    .await
    .map_err(longbox_db::DbError::from)?;

    // Outcome counts — only meaningful for shallow series that
    // have been attempted (matched series are CV-linked and drop
    // out of the shallow filter, so they don't appear in the
    // recent counts; partial_merge stays shallow no — actually
    // partial_merge IS a successful merge so cv_id is set, dropping
    // it out of the shallow set too. So this surface counts
    // refusals + errors on shallow series specifically.)
    let outcome_rows = sqlx::query!(
        r#"SELECT last_enrichment_outcome AS "outcome: String", COUNT(*) AS "n!: i64"
           FROM series
           WHERE cv_id IS NULL AND last_enrichment_outcome IS NOT NULL
           GROUP BY last_enrichment_outcome"#
    )
    .fetch_all(&state.db)
    .await
    .map_err(longbox_db::DbError::from)?;
    let mut recent = RecentOutcomeCounts::default();
    for r in outcome_rows {
        match r.outcome.as_deref() {
            Some("matched") => recent.matched = r.n,
            Some("partial_merge") => recent.partial_merge = r.n,
            Some("no_results") => recent.no_results = r.n,
            Some("low_confidence") => recent.low_confidence = r.n,
            Some("multi_match") => recent.multi_match = r.n,
            Some("year_mismatch") => recent.year_mismatch = r.n,
            Some("count_mismatch") => recent.count_mismatch = r.n,
            Some("collision_disabled") => recent.collision_disabled = r.n,
            Some("cv_id_collision") => recent.cv_id_collision = r.n,
            Some("set_cv_id_race_lost") => recent.set_cv_id_race_lost = r.n,
            Some("error") => recent.error = r.n,
            _ => {}
        }
    }

    Ok(Json(EnrichmentSummary {
        shallow_total,
        awaiting_attempt,
        cooldown_waiting,
        is_running: state.enrichment.is_running(),
        recent_outcomes: recent,
    }))
}

async fn enrich_now(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    if state.enrichment.is_running() {
        return Err(ApiError::Conflict {
            code: "conflict.enrichment_running",
            message: "An enrichment cycle is already running.".into(),
            details: serde_json::Value::Null,
        });
    }
    if !state.enrichment.request_run() {
        return Err(ApiError::Conflict {
            code: "conflict.enrichment_unavailable",
            message: "Enrichment worker is unavailable (refused to start, see logs).".into(),
            details: serde_json::Value::Null,
        });
    }
    Ok(StatusCode::ACCEPTED)
}
