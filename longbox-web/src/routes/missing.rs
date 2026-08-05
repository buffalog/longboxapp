use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_core::IssueNumber;
use serde::{Deserialize, Serialize};
use time::PrimitiveDateTime;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/missing", get(handler))
}

#[derive(Debug, Deserialize)]
struct MissingParams {
    series_id: Option<i64>,
    /// `series` (default) or `cover_date`. Unknown values fall back to
    /// `series`.
    sort: Option<String>,
}

#[derive(Debug, Serialize)]
struct MissingResponse {
    missing: Vec<MissingIssue>,
    total: usize,
}

#[derive(Debug, Serialize)]
struct MissingIssue {
    issue_id: i64,
    number: String,
    title: Option<String>,
    cover_url: Option<String>,
    cover_date: Option<String>,
    /// When the issue was added to the catalog. Frontend uses this as a
    /// "missing for" lower bound when `cover_date` is null.
    issue_created_at: PrimitiveDateTime,
    series: MissingIssueSeries,
}

#[derive(Debug, Serialize)]
struct MissingIssueSeries {
    id: i64,
    title: String,
    sort_title: String,
    start_year: Option<i64>,
}

struct MissingRow {
    issue_id: i64,
    number: String,
    title: Option<String>,
    cover_url: Option<String>,
    cover_date: Option<String>,
    issue_created_at: PrimitiveDateTime,
    series_id: i64,
    series_title: String,
    series_sort_title: String,
    series_start_year: Option<i64>,
}

/// "Missing" = an issue with no present owned file. Mirrors the predicate
/// used by series_repo::list_..._with_counts and routes/stats.rs.
async fn handler(
    State(state): State<AppState>,
    Query(params): Query<MissingParams>,
) -> Result<Json<MissingResponse>, ApiError> {
    let sort = params.sort.as_deref().unwrap_or("series");
    let by_cover_date = sort == "cover_date";

    // Two query variants for the optional series_id filter — sqlx::query!
    // can't dynamically include/exclude a WHERE clause without help. The
    // ORDER BY differs too. Keep both shapes inline for clarity.
    let rows = if let Some(sid) = params.series_id {
        if by_cover_date {
            sqlx::query_as!(
                MissingRow,
                r#"SELECT
                     i.id AS "issue_id!: i64",
                     i.number AS "number!",
                     i.title AS "title?",
                     i.cover_url AS "cover_url?",
                     i.cover_date AS "cover_date?",
                     i.created_at AS "issue_created_at!: time::PrimitiveDateTime",
                     s.id AS "series_id!: i64",
                     s.title AS "series_title!",
                     s.sort_title AS "series_sort_title!",
                     s.start_year AS "series_start_year?: i64"
                   FROM issues i
                   JOIN series s ON i.series_id = s.id
                   WHERE s.id = ?
                     AND NOT EXISTS (
                       SELECT 1 FROM issue_ownership o
                       WHERE o.issue_id = i.id AND o.is_owned = 1
                     )
                   ORDER BY i.cover_date ASC NULLS LAST, i.id ASC"#,
                sid
            )
            .fetch_all(&state.db)
            .await
        } else {
            sqlx::query_as!(
                MissingRow,
                r#"SELECT
                     i.id AS "issue_id!: i64",
                     i.number AS "number!",
                     i.title AS "title?",
                     i.cover_url AS "cover_url?",
                     i.cover_date AS "cover_date?",
                     i.created_at AS "issue_created_at!: time::PrimitiveDateTime",
                     s.id AS "series_id!: i64",
                     s.title AS "series_title!",
                     s.sort_title AS "series_sort_title!",
                     s.start_year AS "series_start_year?: i64"
                   FROM issues i
                   JOIN series s ON i.series_id = s.id
                   WHERE s.id = ?
                     AND NOT EXISTS (
                       SELECT 1 FROM issue_ownership o
                       WHERE o.issue_id = i.id AND o.is_owned = 1
                     )
                   ORDER BY s.sort_title ASC, i.id ASC"#,
                sid
            )
            .fetch_all(&state.db)
            .await
        }
    } else if by_cover_date {
        sqlx::query_as!(
            MissingRow,
            r#"SELECT
                 i.id AS "issue_id!: i64",
                 i.number AS "number!",
                 i.title AS "title?",
                 i.cover_url AS "cover_url?",
                 i.cover_date AS "cover_date?",
                 i.created_at AS "issue_created_at!: time::PrimitiveDateTime",
                 s.id AS "series_id!: i64",
                 s.title AS "series_title!",
                 s.sort_title AS "series_sort_title!",
                 s.start_year AS "series_start_year?: i64"
               FROM issues i
               JOIN series s ON i.series_id = s.id
               WHERE NOT EXISTS (
                 SELECT 1 FROM issue_ownership o
                 WHERE o.issue_id = i.id AND o.is_owned = 1
               )
               ORDER BY i.cover_date ASC NULLS LAST, i.id ASC"#
        )
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as!(
            MissingRow,
            r#"SELECT
                 i.id AS "issue_id!: i64",
                 i.number AS "number!",
                 i.title AS "title?",
                 i.cover_url AS "cover_url?",
                 i.cover_date AS "cover_date?",
                 i.created_at AS "issue_created_at!: time::PrimitiveDateTime",
                 s.id AS "series_id!: i64",
                 s.title AS "series_title!",
                 s.sort_title AS "series_sort_title!",
                 s.start_year AS "series_start_year?: i64"
               FROM issues i
               JOIN series s ON i.series_id = s.id
               WHERE NOT EXISTS (
                 SELECT 1 FROM issue_ownership o
                 WHERE o.issue_id = i.id AND o.is_owned = 1
               )
               ORDER BY s.sort_title ASC, i.id ASC"#
        )
        .fetch_all(&state.db)
        .await
    }
    .map_err(|e| ApiError::Internal {
        message: format!("missing query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;

    // SQL ORDER BY on `s.sort_title, i.id` is a series grouping with
    // arbitrary intra-series order. Apply natural issue-number ordering
    // within each series in Rust so "1, 2, 10, Annual 1" come out right.
    let mut grouped: Vec<MissingRow> = rows;
    if !by_cover_date {
        grouped.sort_by(|a, b| {
            let s = a.series_sort_title.cmp(&b.series_sort_title);
            if s.is_ne() {
                return s;
            }
            IssueNumber::natural_cmp(
                &IssueNumber::new(a.number.clone()),
                &IssueNumber::new(b.number.clone()),
            )
        });
    }

    let missing: Vec<MissingIssue> = grouped
        .into_iter()
        .map(|r| MissingIssue {
            issue_id: r.issue_id,
            number: r.number,
            title: r.title,
            cover_url: r.cover_url,
            cover_date: r.cover_date,
            issue_created_at: r.issue_created_at,
            series: MissingIssueSeries {
                id: r.series_id,
                title: r.series_title,
                sort_title: r.series_sort_title,
                start_year: r.series_start_year,
            },
        })
        .collect();
    let total = missing.len();

    Ok(Json(MissingResponse { missing, total }))
}
