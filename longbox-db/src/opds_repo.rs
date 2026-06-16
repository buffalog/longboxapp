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

/// One issue in a series acquisition feed. `present_filename` is the
/// `path_relative` of a present file backing this issue (preferring an
/// `owned` one); `None` means no owned copy is on disk, so the entry omits
/// its acquisition link. The filename's extension drives the acquisition
/// link's MIME type — no path is ever exposed in the feed XML.
#[derive(Debug, Clone)]
pub struct OpdsIssue {
    pub id: i64,
    pub number: String,
    pub title: Option<String>,
    pub cover_date: Option<String>,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
    pub updated_at: time::PrimitiveDateTime,
    pub present_filename: Option<String>,
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

/// A page of issues for one series, ordered by issue number. Ordering uses
/// the generated `canonical_number` cast to a real so "2" sorts before
/// "10" and zero-padding is ignored; non-numeric numbers (e.g. "Annual 1")
/// sort first as 0, then lexically.
pub async fn list_series_issues_page<'e, E>(
    executor: E,
    series_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<OpdsIssue>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT
             i.id AS "id!: i64",
             i.number,
             i.title,
             i.cover_date,
             i.summary,
             i.cover_url,
             i.updated_at AS "updated_at: time::PrimitiveDateTime",
             (SELECT f.path_relative FROM files f
              WHERE f.issue_id = i.id AND f.is_present = 1
              ORDER BY (f.status = 'owned') DESC, f.id
              LIMIT 1) AS "present_filename?: String"
           FROM issues i
           WHERE i.series_id = ?
           ORDER BY CAST(i.canonical_number AS REAL), i.canonical_number, i.id
           LIMIT ? OFFSET ?"#,
        series_id,
        limit,
        offset
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| OpdsIssue {
            id: r.id,
            number: r.number,
            title: r.title,
            cover_date: r.cover_date,
            summary: r.summary,
            cover_url: r.cover_url,
            updated_at: r.updated_at,
            present_filename: r.present_filename,
        })
        .collect())
}

/// Number of issues in a series.
pub async fn count_series_issues<'e, E>(executor: E, series_id: i64) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM issues WHERE series_id = ?"#,
        series_id
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// A page of series whose title matches `query` (case-insensitive
/// substring), ordered by `sort_title`. Backs OPDS search.
pub async fn search_series_page<'e, E>(
    executor: E,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<SeriesRow>>
where
    E: SqliteExecutor<'e>,
{
    let pattern = like_contains(query);
    let rows = sqlx::query_as!(
        SeriesRow,
        r#"SELECT id AS "id!: i64", cv_id, metron_id, title, sort_title,
                  start_year, publisher, description, cover_url,
                  created_at AS "created_at: _", updated_at AS "updated_at: _"
           FROM series
           WHERE title LIKE ? ESCAPE '\'
           ORDER BY sort_title
           LIMIT ? OFFSET ?"#,
        pattern,
        limit,
        offset
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Number of series matching `query`.
pub async fn count_search_series<'e, E>(executor: E, query: &str) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let pattern = like_contains(query);
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM series WHERE title LIKE ? ESCAPE '\'"#,
        pattern
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Build a `%term%` LIKE pattern, escaping the LIKE metacharacters
/// (`%`, `_`, and the `\` escape itself) in the user's term so they match
/// literally rather than acting as wildcards. Pairs with `ESCAPE '\'`.
fn like_contains(query: &str) -> String {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}
