use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use time::PrimitiveDateTime;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(handler))
}

/// Operational state for monitoring + smoke checks. Kept to **one**
/// DB roundtrip (a single SELECT pulling four aggregates) so a hung
/// or slow DB doesn't compound into a slow health endpoint —
/// `db_ok` is what surfaces that condition.
///
/// `last_scan_at` and `last_enrichment_at` are the most recent
/// completed scan / enrichment-attempt timestamps; null when nothing
/// has run yet (fresh deploy). `enrichment_queue_depth` mirrors the
/// `shallow_total` field on `/api/library/tidy/enrichment-summary`
/// (series with `cv_id IS NULL`) — operators can spot a backlog
/// without crawling that surface.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    db_ok: bool,
    last_scan_at: Option<PrimitiveDateTime>,
    last_enrichment_at: Option<PrimitiveDateTime>,
    enrichment_queue_depth: i64,
    uptime_seconds: i64,
}

async fn handler(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    // Single combined query — all three DB-derived fields land in one
    // roundtrip. `SELECT 1 AS db_ok` is the probe: if the query
    // returns successfully at all, the pool's healthy enough for a
    // read.
    let db_result = sqlx::query!(
        r#"SELECT
             1 AS "db_ok!: i64",
             (SELECT MAX(finished_at) FROM scan_runs WHERE status = 'completed')
               AS "last_scan_at: PrimitiveDateTime",
             (SELECT MAX(last_enrichment_attempt_at) FROM series)
               AS "last_enrichment_at: PrimitiveDateTime",
             (SELECT COUNT(*) FROM series WHERE cv_id IS NULL)
               AS "enrichment_queue_depth!: i64""#
    )
    .fetch_one(&state.db)
    .await;

    let now = time::OffsetDateTime::now_utc();
    let uptime_seconds = (now - state.start_time).whole_seconds().max(0);

    let response = match db_result {
        Ok(row) => HealthResponse {
            status: "ok",
            version: env!("CARGO_PKG_VERSION"),
            db_ok: true,
            last_scan_at: row.last_scan_at,
            last_enrichment_at: row.last_enrichment_at,
            enrichment_queue_depth: row.enrichment_queue_depth,
            uptime_seconds,
        },
        Err(e) => {
            // Don't 500 the health endpoint when the DB hiccups — a
            // monitor scraping this needs to keep getting a response
            // so it can distinguish "process is up, DB is down" from
            // "process is unreachable." Body says `db_ok: false`,
            // every DB-derived field defaults to null/0.
            tracing::warn!(target: "longbox_web", error = %e, "health DB probe failed");
            HealthResponse {
                status: "degraded",
                version: env!("CARGO_PKG_VERSION"),
                db_ok: false,
                last_scan_at: None,
                last_enrichment_at: None,
                enrichment_queue_depth: 0,
                uptime_seconds,
            }
        }
    };

    Ok(Json(response))
}
