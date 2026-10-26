//! Repository for `metron_issue_cache` — one hydrated Metron issue
//! detail per row, written as the forward calendar hydrates a week.
//! Opaque like `metron_calendar_cache_repo`: `payload_json` is a
//! serialized `MetronCalendarItem`, and TTL is the caller's call.

use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct MetronIssueCacheRow {
    pub metron_issue_id: i64,
    pub payload_json: String,
    pub cached_at: PrimitiveDateTime,
}

/// Insert-or-replace one issue's payload and refresh `cached_at`.
pub async fn upsert<'e, E>(executor: E, metron_issue_id: i64, payload_json: &str) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO metron_issue_cache (metron_issue_id, payload_json, cached_at)
           VALUES (?, ?, CURRENT_TIMESTAMP)
           ON CONFLICT(metron_issue_id) DO UPDATE SET
               payload_json = excluded.payload_json,
               cached_at = CURRENT_TIMESTAMP"#,
        metron_issue_id,
        payload_json,
    )
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get<'e, E>(executor: E, metron_issue_id: i64) -> Result<Option<MetronIssueCacheRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        MetronIssueCacheRow,
        r#"SELECT metron_issue_id AS "metron_issue_id!: i64", payload_json,
                  cached_at AS "cached_at: _"
           FROM metron_issue_cache
           WHERE metron_issue_id = ?"#,
        metron_issue_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}
