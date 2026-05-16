use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/stats", get(handler))
}

/// Workspace-wide aggregates for the dashboard. Field names per the Phase A
/// brief: `total_*` for series/issues, `*_files` for the file status
/// rollups. `missing_issues` counts issues with no present owned file
/// matched — semantically distinct from "file rows with is_present=0"
/// (which the dashboard doesn't expose).
#[derive(Debug, Serialize)]
struct Stats {
    total_series: i64,
    total_issues: i64,
    owned_files: i64,
    needs_review_files: i64,
    ignored_files: i64,
    unmatched_files: i64,
    missing_issues: i64,
}

async fn handler(State(state): State<AppState>) -> Result<Json<Stats>, ApiError> {
    let row = sqlx::query!(
        r#"SELECT
             (SELECT COUNT(*) FROM series) AS "total_series!: i64",
             (SELECT COUNT(*) FROM issues) AS "total_issues!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'owned' AND is_present = 1) AS "owned_files!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'needs_review' AND is_present = 1) AS "needs_review_files!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'ignored' AND is_present = 1) AS "ignored_files!: i64",
             (SELECT COUNT(*) FROM files
              WHERE status = 'unmatched' AND is_present = 1) AS "unmatched_files!: i64",
             (SELECT COUNT(*) FROM issues i
              WHERE NOT EXISTS (
                SELECT 1 FROM files f
                WHERE f.issue_id = i.id
                  AND f.status = 'owned'
                  AND f.is_present = 1
              )) AS "missing_issues!: i64""#
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("stats query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    Ok(Json(Stats {
        total_series: row.total_series,
        total_issues: row.total_issues,
        owned_files: row.owned_files,
        needs_review_files: row.needs_review_files,
        ignored_files: row.ignored_files,
        unmatched_files: row.unmatched_files,
        missing_issues: row.missing_issues,
    }))
}
