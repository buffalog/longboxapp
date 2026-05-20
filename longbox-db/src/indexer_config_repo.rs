//! Repository for `indexer_configs` — Newznab indexer connection rows.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct IndexerConfigRow {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    /// Lower number = queried first.
    pub priority: i64,
    pub maxage_days: i64,
    pub created_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewIndexerConfig {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
    pub maxage_days: i64,
}

#[derive(Debug, Clone)]
pub struct IndexerConfigUpdate {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
    pub maxage_days: i64,
}

pub async fn list_all<'e, E>(executor: E) -> Result<Vec<IndexerConfigRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        IndexerConfigRow,
        r#"SELECT id AS "id!: i64", name, base_url, api_key,
                  enabled AS "enabled!: bool", priority AS "priority!: i64",
                  maxage_days AS "maxage_days!: i64", created_at AS "created_at: _"
           FROM indexer_configs
           ORDER BY priority ASC, id ASC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Enabled indexers in priority order — what the pull engine queries.
pub async fn list_enabled<'e, E>(executor: E) -> Result<Vec<IndexerConfigRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        IndexerConfigRow,
        r#"SELECT id AS "id!: i64", name, base_url, api_key,
                  enabled AS "enabled!: bool", priority AS "priority!: i64",
                  maxage_days AS "maxage_days!: i64", created_at AS "created_at: _"
           FROM indexer_configs
           WHERE enabled = 1
           ORDER BY priority ASC, id ASC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn get<'e, E>(executor: E, id: i64) -> Result<Option<IndexerConfigRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        IndexerConfigRow,
        r#"SELECT id AS "id!: i64", name, base_url, api_key,
                  enabled AS "enabled!: bool", priority AS "priority!: i64",
                  maxage_days AS "maxage_days!: i64", created_at AS "created_at: _"
           FROM indexer_configs WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn insert<'e, E>(executor: E, input: NewIndexerConfig) -> Result<IndexerConfigRow>
where
    E: SqliteExecutor<'e>,
{
    let enabled = i64::from(input.enabled);
    let row = sqlx::query_as!(
        IndexerConfigRow,
        r#"INSERT INTO indexer_configs
               (name, base_url, api_key, enabled, priority, maxage_days)
           VALUES (?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", name, base_url, api_key,
                     enabled AS "enabled!: bool", priority AS "priority!: i64",
                     maxage_days AS "maxage_days!: i64", created_at AS "created_at: _""#,
        input.name,
        input.base_url,
        input.api_key,
        enabled,
        input.priority,
        input.maxage_days,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(
    executor: E,
    id: i64,
    patch: IndexerConfigUpdate,
) -> Result<IndexerConfigRow>
where
    E: SqliteExecutor<'e>,
{
    let enabled = i64::from(patch.enabled);
    let row = sqlx::query_as!(
        IndexerConfigRow,
        r#"UPDATE indexer_configs
           SET name = ?, base_url = ?, api_key = ?, enabled = ?,
               priority = ?, maxage_days = ?
           WHERE id = ?
           RETURNING id AS "id!: i64", name, base_url, api_key,
                     enabled AS "enabled!: bool", priority AS "priority!: i64",
                     maxage_days AS "maxage_days!: i64", created_at AS "created_at: _""#,
        patch.name,
        patch.base_url,
        patch.api_key,
        enabled,
        patch.priority,
        patch.maxage_days,
        id,
    )
    .fetch_optional(executor)
    .await?;
    row.ok_or(DbError::NotFound)
}

pub async fn delete<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM indexer_configs WHERE id = ?"#, id)
        .execute(executor)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}
