//! Repository for `metron_calendar_cache` — cached Metron forward-week
//! calendar payloads, keyed by `(date_from, date_to)`.
//!
//! Mirror of `release_cache_repo`'s shape, in its own namespace because
//! the payload projections are different (Metron-flat vs CV-flat) and
//! conflating sources in one table would force a wrapper enum on every
//! read.
//!
//! TTL is read-time policy, not stored — the row carries `cached_at`,
//! and the calendar handler compares against
//! `metron_calendar_cache_ttl_hours` from the settings table per
//! request. The cache is opaque to this repo: `payload_json` is just a
//! string, and the type it deserializes into (`Vec<MetronCalendarItem>`)
//! lives in `longbox-metron`, kept out of this crate to avoid a
//! cross-crate dependency.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct MetronCalendarCacheRow {
    pub id: i64,
    pub date_from: String,
    pub date_to: String,
    /// Serialized `Vec<MetronCalendarItem>` — fully hydrated, every row
    /// has `publisher: Some(...)` resolved before this is written. The
    /// calendar handler deserializes and merges directly into the
    /// response.
    pub payload_json: String,
    pub cached_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewMetronCalendarCacheEntry {
    pub date_from: String,
    pub date_to: String,
    pub payload_json: String,
}

/// Insert-or-replace a cache entry. The `UNIQUE(date_from, date_to)`
/// constraint on the table makes a re-cache of the same window
/// overwrite the prior payload and refresh `cached_at`.
pub async fn upsert<'e, E>(
    executor: E,
    input: NewMetronCalendarCacheEntry,
) -> Result<MetronCalendarCacheRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        MetronCalendarCacheRow,
        r#"INSERT INTO metron_calendar_cache
               (date_from, date_to, payload_json, cached_at)
           VALUES (?, ?, ?, CURRENT_TIMESTAMP)
           ON CONFLICT(date_from, date_to) DO UPDATE SET
               payload_json = excluded.payload_json,
               cached_at = CURRENT_TIMESTAMP
           RETURNING id AS "id!: i64", date_from, date_to,
                     payload_json, cached_at AS "cached_at: _""#,
        input.date_from,
        input.date_to,
        input.payload_json,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Fetch a cache entry by exact key. Staleness is the caller's call —
/// compare `cached_at` against `metron_calendar_cache_ttl_hours` from
/// the settings table.
pub async fn get<'e, E>(
    executor: E,
    date_from: &str,
    date_to: &str,
) -> Result<Option<MetronCalendarCacheRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        MetronCalendarCacheRow,
        r#"SELECT id AS "id!: i64", date_from, date_to,
                  payload_json, cached_at AS "cached_at: _"
           FROM metron_calendar_cache
           WHERE date_from = ? AND date_to = ?"#,
        date_from,
        date_to,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}
