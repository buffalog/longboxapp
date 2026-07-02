//! Repository for `cv_volume_cache` — per-CV-volume metadata cache
//! (publisher, description, start_year) keyed by cv_volume_id.
//!
//! The calendar's publisher grouping (Item E) uses this as a
//! second-source fallback when a calendar row's cv_volume_id doesn't
//! map to a tracked series: series JOIN first, then cv_volume_cache.
//! The enrichment worker drains pending rows (fetched_at IS NULL) as
//! its third pass, after the shallow + refresh passes that came in
//! 6c.5 piece 1.
//!
//! The cache is permanent — publisher data is structurally stable
//! and the table is bounded by volumes that have appeared in a
//! calendar window LongBox has loaded.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::Result;

/// Public view of a cv_volume_cache row.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct CvVolumeCacheRow {
    pub cv_volume_id: i64,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub start_year: Option<i64>,
    pub cover_url: Option<String>,
    pub fetched_at: Option<PrimitiveDateTime>,
    pub first_seen_at: PrimitiveDateTime,
}

/// Worker's pending-candidate shape — just the id is needed to call
/// fetch_volume; the rest of the row is overwritten on success.
#[derive(Debug, Clone, PartialEq)]
pub struct CvVolumePending {
    pub cv_volume_id: i64,
}

/// One row of the calendar-fallback view: (cv_volume_id, publisher).
/// The calendar handler builds a `HashMap<i64, Option<String>>` from a
/// `Vec<CvVolumePublisherEntry>` so the fallback lookup inside the
/// per-item loop is one HashMap probe each, not 99 DB queries.
#[derive(Debug, Clone, PartialEq)]
pub struct CvVolumePublisherEntry {
    pub cv_volume_id: i64,
    pub publisher: Option<String>,
}

/// All cache rows, projected to (cv_volume_id, publisher). The calendar
/// handler reads this once per request to build its fallback HashMap —
/// one query, then in-memory lookups for the per-item loop.
///
/// Rows that are pending (fetched_at IS NULL) carry publisher = NULL,
/// which the calendar treats identically to "no cache entry": the row
/// groups under "Unknown Publisher" until the worker fills it.
pub async fn list_all_publishers<'e, E>(executor: E) -> Result<Vec<CvVolumePublisherEntry>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        CvVolumePublisherEntry,
        r#"SELECT cv_volume_id AS "cv_volume_id!: i64",
                  publisher
           FROM cv_volume_cache"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Discovery-facing metadata for one cached volume. `publisher`/`start_year`/
/// `cover_url` are all `None` for a still-pending row.
#[derive(Debug, Clone, PartialEq)]
pub struct CvVolumeMeta {
    pub cv_volume_id: i64,
    pub publisher: Option<String>,
    pub start_year: Option<i64>,
    pub cover_url: Option<String>,
}

/// All cache rows projected to discovery metadata. Discovery reads this once
/// per request and builds a `HashMap<cv_volume_id, CvVolumeMeta>` for in-memory
/// joins — mirrors `list_all_publishers` (the calendar path).
// ponytail: fetch-all + HashMap like the calendar path; if the cache grows to
// 100k+ rows, switch to a batched IN query keyed on the discovered ids.
pub async fn list_metadata_all<'e, E>(executor: E) -> Result<Vec<CvVolumeMeta>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        CvVolumeMeta,
        r#"SELECT cv_volume_id AS "cv_volume_id!: i64",
                  publisher,
                  start_year,
                  cover_url
           FROM cv_volume_cache"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Bulk-queue cv_volume_ids as pending rows. INSERT OR IGNORE on the
/// primary key means: rows already present (pending or already
/// fetched) are no-ops; only genuinely new ids land as pending. The
/// calendar handler calls this with the ids it just saw that aren't
/// in the cache map yet — synchronously, inside the request — to
/// produce the worker's queue.
///
/// Wrapped in a single transaction so the 99-or-so per-week inserts
/// fsync once, not 99 times.
pub async fn bulk_queue_pending(pool: &crate::Pool, cv_volume_ids: &[i64]) -> Result<u64> {
    if cv_volume_ids.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut inserted = 0u64;
    for id in cv_volume_ids {
        let result = sqlx::query!(
            r#"INSERT OR IGNORE INTO cv_volume_cache (cv_volume_id) VALUES (?)"#,
            id
        )
        .execute(&mut *tx)
        .await?;
        inserted += result.rows_affected();
    }
    tx.commit().await?;
    Ok(inserted)
}

/// List pending cache rows (fetched_at IS NULL). The worker's third
/// pass drains this set: one fetch_volume per id, then
/// [`mark_fetched`] with the resolved publisher / description /
/// start_year. Order is FIFO by first_seen_at — older queue entries
/// (more likely to be hit by repeat calendar loads) drain first.
pub async fn list_pending<'e, E>(executor: E) -> Result<Vec<CvVolumePending>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        CvVolumePending,
        r#"SELECT cv_volume_id AS "cv_volume_id!: i64"
           FROM cv_volume_cache
           WHERE fetched_at IS NULL
           ORDER BY first_seen_at ASC, cv_volume_id ASC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Write the resolved volume detail and mark the row fetched. Called
/// by the worker after a successful fetch_volume. A row where CV
/// genuinely doesn't return publisher still gets fetched_at set — the
/// worker uses fetched_at, not publisher non-null, as the
/// "I've done the work" signal so it doesn't re-attempt.
pub async fn mark_fetched<'e, E>(
    executor: E,
    cv_volume_id: i64,
    publisher: Option<&str>,
    description: Option<&str>,
    start_year: Option<i32>,
    cover_url: Option<&str>,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let start_year_i64 = start_year.map(i64::from);
    let result = sqlx::query!(
        r#"UPDATE cv_volume_cache
           SET publisher = ?, description = ?, start_year = ?, cover_url = ?,
               fetched_at = CURRENT_TIMESTAMP
           WHERE cv_volume_id = ?"#,
        publisher,
        description,
        start_year_i64,
        cover_url,
        cv_volume_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Fetch one row by primary key. Used by tests; the production code
/// paths read via [`list_all_publishers`] and [`list_pending`].
#[allow(dead_code)]
pub async fn find_by_id<'e, E>(executor: E, cv_volume_id: i64) -> Result<Option<CvVolumeCacheRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        CvVolumeCacheRow,
        r#"SELECT cv_volume_id AS "cv_volume_id!: i64",
                  publisher,
                  description,
                  start_year,
                  cover_url,
                  fetched_at AS "fetched_at: _",
                  first_seen_at AS "first_seen_at!: _"
           FROM cv_volume_cache
           WHERE cv_volume_id = ?"#,
        cv_volume_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}
