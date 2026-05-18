use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesRow {
    pub id: i64,
    pub cv_id: Option<i64>,
    pub metron_id: Option<String>,
    pub title: String,
    pub sort_title: String,
    pub start_year: Option<i64>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_at: PrimitiveDateTime,
    pub updated_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewSeries {
    pub cv_id: Option<i64>,
    pub metron_id: Option<String>,
    pub title: String,
    pub sort_title: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

/// Fields that can be overwritten via `update`. `cv_id` and `metron_id` are
/// identity columns and not updatable here; use a new insert if you need a
/// different external ID.
#[derive(Debug, Clone)]
pub struct SeriesUpdate {
    pub title: String,
    pub sort_title: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn find_by_cv_id<'e, E>(executor: E, cv_id: i64) -> Result<Option<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series WHERE cv_id = ?"#,
        cv_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn find_by_metron_id<'e, E>(
    executor: E,
    metron_id: &str,
) -> Result<Option<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series WHERE metron_id = ?"#,
        metron_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Series row plus the per-status issue counts the web layer's series list
/// surface needs. Single JOIN'd query — no N+1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesWithCounts {
    #[serde(flatten)]
    pub series: SeriesRow,
    pub total_count: i64,
    pub owned_count: i64,
    pub needs_review_count: i64,
    pub ignored_count: i64,
    pub unmatched_count: i64,
    pub missing_count: i64,
}

/// Like [`find_all`] but augments each row with the per-status issue
/// counts. `missing_count` is "issues for which no present owned file
/// exists" — not derivable from `total_count - owned_count`, because that
/// conflates with needs_review and ignored.
pub async fn find_all_with_counts<'e, E>(
    executor: E,
) -> Result<Vec<SeriesWithCounts>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT
             s.id AS "id!: i64",
             s.cv_id, s.metron_id, s.title, s.sort_title, s.start_year,
             s.publisher, s.description, s.cover_url,
             s.created_at AS "created_at: time::PrimitiveDateTime",
             s.updated_at AS "updated_at: time::PrimitiveDateTime",
             COUNT(DISTINCT i.id) AS "total_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'owned' AND f.is_present = 1
               THEN i.id END) AS "owned_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'needs_review' AND f.is_present = 1
               THEN i.id END) AS "needs_review_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'ignored' AND f.is_present = 1
               THEN i.id END) AS "ignored_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'unmatched' AND f.is_present = 1
               THEN i.id END) AS "unmatched_count!: i64",
             COUNT(DISTINCT CASE
               WHEN NOT EXISTS (
                 SELECT 1 FROM files f2
                 WHERE f2.issue_id = i.id
                   AND f2.status = 'owned'
                   AND f2.is_present = 1
               )
               THEN i.id END) AS "missing_count!: i64"
           FROM series s
           LEFT JOIN issues i ON i.series_id = s.id
           LEFT JOIN files f ON f.issue_id = i.id
           GROUP BY s.id
           ORDER BY s.sort_title"#
    )
    .fetch_all(executor)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesWithCounts {
            series: SeriesRow {
                id: r.id,
                cv_id: r.cv_id,
                metron_id: r.metron_id,
                title: r.title,
                sort_title: r.sort_title,
                start_year: r.start_year,
                publisher: r.publisher,
                description: r.description,
                cover_url: r.cover_url,
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
            total_count: r.total_count,
            owned_count: r.owned_count,
            needs_review_count: r.needs_review_count,
            ignored_count: r.ignored_count,
            unmatched_count: r.unmatched_count,
            missing_count: r.missing_count,
        })
        .collect())
}

/// Like [`find_all_with_counts`] but newest-first by `created_at` and
/// limited. Used by the dashboard activity feed's "Recently added series"
/// section.
pub async fn list_recent_with_counts<'e, E>(
    executor: E,
    limit: u32,
) -> Result<Vec<SeriesWithCounts>>
where
    E: SqliteExecutor<'e>,
{
    let limit_i64 = i64::from(limit);
    let rows = sqlx::query!(
        r#"SELECT
             s.id AS "id!: i64",
             s.cv_id, s.metron_id, s.title, s.sort_title, s.start_year,
             s.publisher, s.description, s.cover_url,
             s.created_at AS "created_at: time::PrimitiveDateTime",
             s.updated_at AS "updated_at: time::PrimitiveDateTime",
             COUNT(DISTINCT i.id) AS "total_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'owned' AND f.is_present = 1
               THEN i.id END) AS "owned_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'needs_review' AND f.is_present = 1
               THEN i.id END) AS "needs_review_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'ignored' AND f.is_present = 1
               THEN i.id END) AS "ignored_count!: i64",
             COUNT(DISTINCT CASE
               WHEN f.status = 'unmatched' AND f.is_present = 1
               THEN i.id END) AS "unmatched_count!: i64",
             COUNT(DISTINCT CASE
               WHEN NOT EXISTS (
                 SELECT 1 FROM files f2
                 WHERE f2.issue_id = i.id
                   AND f2.status = 'owned'
                   AND f2.is_present = 1
               )
               THEN i.id END) AS "missing_count!: i64"
           FROM series s
           LEFT JOIN issues i ON i.series_id = s.id
           LEFT JOIN files f ON f.issue_id = i.id
           GROUP BY s.id
           ORDER BY s.created_at DESC, s.id DESC
           LIMIT ?"#,
        limit_i64
    )
    .fetch_all(executor)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesWithCounts {
            series: SeriesRow {
                id: r.id,
                cv_id: r.cv_id,
                metron_id: r.metron_id,
                title: r.title,
                sort_title: r.sort_title,
                start_year: r.start_year,
                publisher: r.publisher,
                description: r.description,
                cover_url: r.cover_url,
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
            total_count: r.total_count,
            owned_count: r.owned_count,
            needs_review_count: r.needs_review_count,
            ignored_count: r.ignored_count,
            unmatched_count: r.unmatched_count,
            missing_count: r.missing_count,
        })
        .collect())
}

pub async fn find_all<'e, E>(executor: E) -> Result<Vec<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series ORDER BY sort_title"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn insert<'e, E>(executor: E, input: NewSeries) -> Result<SeriesRow>
where
    E: SqliteExecutor<'e>,
{
    let start_year = input.start_year.map(i64::from);
    let row = sqlx::query_as!(
        SeriesRow,
        r#"INSERT INTO series (cv_id, metron_id, title, sort_title, start_year,
                               publisher, description, cover_url)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", cv_id, metron_id, title, sort_title,
                     start_year, publisher, description, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        input.cv_id,
        input.metron_id,
        input.title,
        input.sort_title,
        start_year,
        input.publisher,
        input.description,
        input.cover_url
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(executor: E, id: i64, patch: SeriesUpdate) -> Result<SeriesRow>
where
    E: SqliteExecutor<'e>,
{
    let start_year = patch.start_year.map(i64::from);
    let row = sqlx::query_as!(
        SeriesRow,
        r#"UPDATE series
           SET title = ?, sort_title = ?, start_year = ?,
               publisher = ?, description = ?, cover_url = ?,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?
           RETURNING id AS "id!: i64", cv_id, metron_id, title, sort_title,
                     start_year, publisher, description, cover_url,
                     created_at AS "created_at: _", updated_at AS "updated_at: _""#,
        patch.title,
        patch.sort_title,
        start_year,
        patch.publisher,
        patch.description,
        patch.cover_url,
        id
    )
    .fetch_optional(executor)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(row)
}
