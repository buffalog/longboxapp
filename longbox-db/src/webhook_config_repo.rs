//! Repository for `webhook_configs` — webhook notification targets.
//!
//! `event_mask` is an INTEGER bitset; a webhook fires for an event
//! only when the corresponding bit is set. Bit assignments are the
//! `EVENT_*` constants below — stable, append-only (new events take
//! the next free bit).

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

/// A pull completed and the file was catalogued.
pub const EVENT_PULL_SUCCEEDED: i64 = 1 << 0;
/// A pull failed permanently (after retry exhaustion).
pub const EVENT_PULL_FAILED: i64 = 1 << 1;
/// New solicitations appeared for a pulled series.
pub const EVENT_NEW_SOLICITATIONS: i64 = 1 << 2;
/// The pull engine itself errored (config / indexer-wide failure).
pub const EVENT_PULL_ENGINE_ERROR: i64 = 1 << 3;

/// Mask with every defined event bit set.
pub const EVENT_MASK_ALL: i64 =
    EVENT_PULL_SUCCEEDED | EVENT_PULL_FAILED | EVENT_NEW_SOLICITATIONS | EVENT_PULL_ENGINE_ERROR;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct WebhookConfigRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    /// Bitset of `EVENT_*` flags this webhook subscribes to.
    pub event_mask: i64,
    pub enabled: bool,
    pub created_at: PrimitiveDateTime,
}

impl WebhookConfigRow {
    /// True if this webhook subscribes to the given event bit.
    pub fn subscribes_to(&self, event: i64) -> bool {
        self.event_mask & event != 0
    }
}

#[derive(Debug, Clone)]
pub struct NewWebhookConfig {
    pub name: String,
    pub url: String,
    pub event_mask: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct WebhookConfigUpdate {
    pub name: String,
    pub url: String,
    pub event_mask: i64,
    pub enabled: bool,
}

pub async fn list_all<'e, E>(executor: E) -> Result<Vec<WebhookConfigRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        WebhookConfigRow,
        r#"SELECT id AS "id!: i64", name, url, event_mask AS "event_mask!: i64",
                  enabled AS "enabled!: bool", created_at AS "created_at: _"
           FROM webhook_configs ORDER BY name COLLATE NOCASE"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Enabled webhooks subscribed to `event` — the dispatch fan-out set.
pub async fn list_subscribed<'e, E>(executor: E, event: i64) -> Result<Vec<WebhookConfigRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        WebhookConfigRow,
        r#"SELECT id AS "id!: i64", name, url, event_mask AS "event_mask!: i64",
                  enabled AS "enabled!: bool", created_at AS "created_at: _"
           FROM webhook_configs
           WHERE enabled = 1 AND (event_mask & ?) != 0
           ORDER BY name COLLATE NOCASE"#,
        event
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn get<'e, E>(executor: E, id: i64) -> Result<Option<WebhookConfigRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        WebhookConfigRow,
        r#"SELECT id AS "id!: i64", name, url, event_mask AS "event_mask!: i64",
                  enabled AS "enabled!: bool", created_at AS "created_at: _"
           FROM webhook_configs WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn insert<'e, E>(executor: E, input: NewWebhookConfig) -> Result<WebhookConfigRow>
where
    E: SqliteExecutor<'e>,
{
    let enabled = i64::from(input.enabled);
    let row = sqlx::query_as!(
        WebhookConfigRow,
        r#"INSERT INTO webhook_configs (name, url, event_mask, enabled)
           VALUES (?, ?, ?, ?)
           RETURNING id AS "id!: i64", name, url, event_mask AS "event_mask!: i64",
                     enabled AS "enabled!: bool", created_at AS "created_at: _""#,
        input.name,
        input.url,
        input.event_mask,
        enabled,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(
    executor: E,
    id: i64,
    patch: WebhookConfigUpdate,
) -> Result<WebhookConfigRow>
where
    E: SqliteExecutor<'e>,
{
    let enabled = i64::from(patch.enabled);
    let row = sqlx::query_as!(
        WebhookConfigRow,
        r#"UPDATE webhook_configs
           SET name = ?, url = ?, event_mask = ?, enabled = ?
           WHERE id = ?
           RETURNING id AS "id!: i64", name, url, event_mask AS "event_mask!: i64",
                     enabled AS "enabled!: bool", created_at AS "created_at: _""#,
        patch.name,
        patch.url,
        patch.event_mask,
        enabled,
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
    let result = sqlx::query!(r#"DELETE FROM webhook_configs WHERE id = ?"#, id)
        .execute(executor)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}
