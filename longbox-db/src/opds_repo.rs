//! Read queries backing the OPDS catalog feeds.
//!
//! OPDS exposes the *local* library: paginated series lists (all, and
//! per-publisher) and the derived publisher list. Publishers are not a
//! table — they're the distinct non-empty `series.publisher` values — so
//! the publisher list and its counts are computed here rather than joined
//! from a `publishers` entity. All series projections reuse
//! [`SeriesRow`](crate::series_repo::SeriesRow).

use sqlx::SqliteExecutor;

use crate::error::Result;
use crate::series_repo::SeriesRow;

/// One publisher in the OPDS publisher navigation feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherEntry {
    pub name: String,
    pub series_count: i64,
}

/// A page of all series, ordered by `sort_title`.
pub async fn list_series_page<'e, E>(executor: E, limit: i64, offset: i64) -> Result<Vec<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series
           ORDER BY sort_title
           LIMIT ? OFFSET ?"#,
        limit,
        offset
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Total number of series (for OpenSearch `totalResults`).
pub async fn count_series<'e, E>(executor: E) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(r#"SELECT COUNT(*) AS "count!: i64" FROM series"#)
        .fetch_one(executor)
        .await?;
    Ok(row.count)
}

/// A page of series for one publisher, ordered by `sort_title`.
pub async fn list_series_by_publisher_page<'e, E>(
    executor: E,
    publisher: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series
           WHERE publisher = ?
           ORDER BY sort_title
           LIMIT ? OFFSET ?"#,
        publisher,
        limit,
        offset
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Number of series for one publisher.
pub async fn count_series_by_publisher<'e, E>(executor: E, publisher: &str) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM series WHERE publisher = ?"#,
        publisher
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// A page of distinct publishers (non-null, non-empty) with their series
/// counts, ordered by name.
pub async fn list_publishers_page<'e, E>(
    executor: E,
    limit: i64,
    offset: i64,
) -> Result<Vec<PublisherEntry>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT publisher AS "name!: String", COUNT(*) AS "series_count!: i64"
           FROM series
           WHERE publisher IS NOT NULL AND publisher <> ''
           GROUP BY publisher
           ORDER BY publisher
           LIMIT ? OFFSET ?"#,
        limit,
        offset
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PublisherEntry {
            name: r.name,
            series_count: r.series_count,
        })
        .collect())
}

/// Number of distinct non-empty publishers (for OpenSearch `totalResults`).
pub async fn count_publishers<'e, E>(executor: E) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM (
             SELECT 1 FROM series
             WHERE publisher IS NOT NULL AND publisher <> ''
             GROUP BY publisher
           )"#
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}
