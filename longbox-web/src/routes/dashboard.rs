use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_db::{series_repo, SeriesWithCounts};
use serde::{Deserialize, Serialize};
use time::PrimitiveDateTime;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/dashboard/activity", get(activity))
}

#[derive(Debug, Deserialize)]
struct ActivityParams {
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct ActivityResponse {
    /// Most recent series added to the watchlist, newest first. Full
    /// `SeriesWithCounts` shape so the dashboard card can render the
    /// owned/total badge without an extra fetch.
    recent_series: Vec<SeriesWithCounts>,
    /// Most recent file → issue matches. File-centric (one row per
    /// match event); the same issue can appear twice if two files were
    /// matched in quick succession.
    recent_matches: Vec<RecentMatch>,
}

#[derive(Debug, Serialize)]
struct RecentMatch {
    file_id: i64,
    path_relative: String,
    matched_at: PrimitiveDateTime,
    issue: RecentMatchIssue,
    series: RecentMatchSeries,
}

#[derive(Debug, Serialize)]
struct RecentMatchIssue {
    id: i64,
    number: String,
    title: Option<String>,
    cover_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct RecentMatchSeries {
    id: i64,
    title: String,
    start_year: Option<i64>,
}

/// Flat row shape for the recent-matches JOIN.
struct RecentMatchRow {
    file_id: i64,
    path_relative: String,
    matched_at: PrimitiveDateTime,
    issue_id: i64,
    issue_number: String,
    issue_title: Option<String>,
    issue_cover_url: Option<String>,
    series_id: i64,
    series_title: String,
    series_start_year: Option<i64>,
}

async fn activity(
    State(state): State<AppState>,
    Query(params): Query<ActivityParams>,
) -> Result<Json<ActivityResponse>, ApiError> {
    let limit = params.limit.unwrap_or(6);
    if limit == 0 || limit > 50 {
        return Err(ApiError::BadRequest {
            message: "limit must be in [1, 50]".into(),
        });
    }

    let recent_series = series_repo::list_recent_with_counts(&state.db, limit).await?;

    let limit_i64 = i64::from(limit);
    // `_unsorted/` is the junk drawer the post-processor parks
    // pending-intervention files in until the user resolves them.
    // Surfacing those raw paths on the dashboard's "Recently
    // completed issues" feed reads as catalog noise — they're
    // matched issues but the path is provisional. Filter with
    // byte-exact substr equality (10 chars including the trailing
    // slash) rather than LIKE — LIKE's `_` wildcard would treat the
    // leading underscore as "any one char" without an ESCAPE clause.
    let match_rows = sqlx::query_as!(
        RecentMatchRow,
        r#"SELECT
             f.id AS "file_id!: i64",
             f.path_relative,
             f.matched_at AS "matched_at!: time::PrimitiveDateTime",
             i.id AS "issue_id!: i64",
             i.number AS "issue_number!",
             i.title AS "issue_title?",
             i.cover_url AS "issue_cover_url?",
             s.id AS "series_id!: i64",
             s.title AS "series_title!",
             s.start_year AS "series_start_year?: i64"
           FROM files f
           JOIN issues i ON f.issue_id = i.id
           JOIN series s ON i.series_id = s.id
           WHERE f.matched_at IS NOT NULL
             AND substr(f.path_relative, 1, 10) != '_unsorted/'
           ORDER BY f.matched_at DESC, f.id DESC
           LIMIT ?"#,
        limit_i64
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("dashboard activity query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;

    let recent_matches: Vec<RecentMatch> = match_rows
        .into_iter()
        .map(|r| RecentMatch {
            file_id: r.file_id,
            path_relative: r.path_relative,
            matched_at: r.matched_at,
            issue: RecentMatchIssue {
                id: r.issue_id,
                number: r.issue_number,
                title: r.issue_title,
                cover_url: r.issue_cover_url,
            },
            series: RecentMatchSeries {
                id: r.series_id,
                title: r.series_title,
                start_year: r.series_start_year,
            },
        })
        .collect();

    Ok(Json(ActivityResponse {
        recent_series,
        recent_matches,
    }))
}
