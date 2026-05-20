//! Repository for `cv_release_cache` — cached ComicVine release-calendar
//! query results, keyed by date range + publisher.
//!
//! TTL is a read-time policy, not stored: the row carries `cached_at`,
//! and each caller decides its own staleness threshold (the
//! release-calendar view uses a short TTL, the pull engine a daily
//! one). [`prune_stale`] sweeps rows older than a cutoff.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct ReleaseCacheRow {
    pub id: i64,
    pub date_from: String,
    pub date_to: String,
    /// Empty string = "all publishers".
    pub publisher: String,
    /// The CV projection serialized as JSON.
    pub payload_json: String,
    pub cached_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewReleaseCacheEntry {
    pub date_from: String,
    pub date_to: String,
    pub publisher: String,
    pub payload_json: String,
}

/// Insert-or-replace a cache entry. The `UNIQUE(date_from, date_to,
/// publisher)` constraint makes a re-cache of the same query overwrite
/// the prior payload and refresh `cached_at`.
pub async fn upsert<'e, E>(executor: E, input: NewReleaseCacheEntry) -> Result<ReleaseCacheRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        ReleaseCacheRow,
        r#"INSERT INTO cv_release_cache
               (date_from, date_to, publisher, payload_json, cached_at)
           VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP)
           ON CONFLICT(date_from, date_to, publisher) DO UPDATE SET
               payload_json = excluded.payload_json,
               cached_at = CURRENT_TIMESTAMP
           RETURNING id AS "id!: i64", date_from, date_to, publisher,
                     payload_json, cached_at AS "cached_at: _""#,
        input.date_from,
        input.date_to,
        input.publisher,
        input.payload_json,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Fetch a cache entry by its exact key. Staleness is the caller's
/// call — compare `cached_at` against the relevant TTL.
pub async fn get<'e, E>(
    executor: E,
    date_from: &str,
    date_to: &str,
    publisher: &str,
) -> Result<Option<ReleaseCacheRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        ReleaseCacheRow,
        r#"SELECT id AS "id!: i64", date_from, date_to, publisher,
                  payload_json, cached_at AS "cached_at: _"
           FROM cv_release_cache
           WHERE date_from = ? AND date_to = ? AND publisher = ?"#,
        date_from,
        date_to,
        publisher
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Delete cache rows older than `cutoff`. Returns the number removed.
pub async fn prune_stale<'e, E>(executor: E, cutoff: PrimitiveDateTime) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"DELETE FROM cv_release_cache WHERE cached_at < ?"#,
        cutoff
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
