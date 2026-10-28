use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use longbox_db::series_repo;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/stats", get(handler))
}

/// Workspace-wide aggregates for the dashboard. `total_*` are
/// series/issue rollups; `*_files` are the per-status file counts;
/// `missing_issues` counts issues with no present owned file (distinct
/// from "file rows with is_present=0") that aren't solicited — the
/// effective release date `COALESCE(store_date, cover_date)` isn't a full
/// date today or later, outside collected-edition volumes
/// (`series_repo::collected_edition_ids`).
///
/// The pull/needs-attention counts here are what previously required
/// three additional HTTP round-trips per dashboard load (one each for
/// `/pull-list`, `/needs-attention/pull-failures`, `/postprocess/pending`).
/// They're folded into one SQL query + one O(1) cache read so the
/// dashboard fans out a single GET.
#[derive(Debug, Serialize)]
struct Stats {
    total_series: i64,
    total_issues: i64,
    owned_files: i64,
    needs_review_files: i64,
    ignored_files: i64,
    unmatched_files: i64,
    missing_issues: i64,
    /// Distinct series with at least one missing issue. Used by the
    /// dashboard's "missing" card to compose "X issues missing across Y
    /// series." Always ≤ total_series.
    series_with_missing: i64,
    /// Series subscribed for auto-download — sourced from the
    /// `pull_list` table. Powers the dashboard "Pull list" tile.
    pull_list_count: i64,
    /// Issues whose most recent pull attempt is in a failure-class
    /// state (`'failed'` or `'mismatched'`). Mirrors the `list_failed`
    /// query in pull_attempt_repo — one row per issue, latest-attempt
    /// semantics — but returns just the count.
    pull_failures_count: i64,
    /// Length of the in-memory post-processor pending-intervention
    /// cache. O(1) read from the cache's read lock. Counts files
    /// stuck after a post-processor attempt; surfaces on the dashboard
    /// "Needs attention" tile alongside `pull_failures_count`.
    pending_interventions_count: i64,
}

async fn handler(State(state): State<AppState>) -> Result<Json<Stats>, ApiError> {
    let row = sqlx::query!(
        r#"SELECT
             -- "Real" series: enriched (cv_id non-null) OR carrying
             -- at least one owned, present file. Excludes
             -- unenriched, file-less phantoms that linger from
             -- bulk-converted folders and aren't actionable
             -- catalog state.
             (SELECT COUNT(*) FROM series s
              WHERE s.cv_id IS NOT NULL
                 OR EXISTS (
                   SELECT 1 FROM files f
                   JOIN issues i ON f.issue_id = i.id
                   WHERE i.series_id = s.id
                     AND f.status = 'owned'
                     AND f.is_present = 1
                 )) AS "total_series!: i64",
             (SELECT COUNT(*) FROM issues) AS "total_issues!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'owned' AND is_present = 1) AS "owned_files!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'needs_review' AND is_present = 1) AS "needs_review_files!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'ignored' AND is_present = 1) AS "ignored_files!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'unmatched' AND is_present = 1) AS "unmatched_files!: i64",
             (SELECT COUNT(*) FROM pull_list) AS "pull_list_count!: i64",
             -- Failure-class pull attempts, one row per issue (latest
             -- attempt semantics — matches `pull_attempt_repo::list_failed`).
             -- Correlated MAX subquery picks the most recent attempt for
             -- each (series, issue) pair; the IN gate filters that latest
             -- row to the two failure states.
             (SELECT COUNT(*) FROM pull_attempts pa
              WHERE pa.status IN ('failed', 'mismatched')
                AND pa.id = (
                  SELECT MAX(p2.id) FROM pull_attempts p2
                  WHERE p2.series_id = pa.series_id
                    AND p2.issue_id = pa.issue_id
                )) AS "pull_failures_count!: i64""#
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("stats query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    // Missing per series, so collected-edition volumes can be left out:
    // the guard that names them is Rust, not SQL.
    let missing = sqlx::query!(
        r#"SELECT i.series_id AS "series_id!: i64", COUNT(*) AS "n!: i64"
           FROM issues i
           WHERE NOT EXISTS (
             SELECT 1 FROM files f
             WHERE f.issue_id = i.id
               AND f.status = 'owned'
               AND f.is_present = 1
           )
             -- Not solicited: same split as series_repo's
             -- solicited_count and the frontend's isSolicited, so
             -- undated issues still count as missing.
             AND (length(COALESCE(i.store_date, i.cover_date)) IS NOT 10
                  OR COALESCE(i.store_date, i.cover_date) < date('now'))
           GROUP BY i.series_id"#
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("stats missing query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    let trades = series_repo::collected_edition_ids(&state.db).await?;
    let missing: Vec<i64> = missing
        .iter()
        .filter(|r| !trades.contains(&r.series_id))
        .map(|r| r.n)
        .collect();
    // The post-processor's pending-intervention count lives in an
    // in-memory cache, not the DB — O(1) read under the cache's
    // RwLock. Folded into the same response so the dashboard's
    // "Needs attention" tile aggregates from one HTTP call.
    let pending_interventions_count = i64::try_from(state.pending_cache.len()).unwrap_or(i64::MAX);
    Ok(Json(Stats {
        total_series: row.total_series,
        total_issues: row.total_issues,
        owned_files: row.owned_files,
        needs_review_files: row.needs_review_files,
        ignored_files: row.ignored_files,
        unmatched_files: row.unmatched_files,
        missing_issues: missing.iter().sum(),
        series_with_missing: i64::try_from(missing.len()).unwrap_or(i64::MAX),
        pull_list_count: row.pull_list_count,
        pull_failures_count: row.pull_failures_count,
        pending_interventions_count,
    }))
}
