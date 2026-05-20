//! Repository for `downloader_config` — the single-row Usenet
//! downloader configuration (SABnzbd or NZBGet).

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct DownloaderConfigRow {
    pub id: i64,
    /// `sab` | `nzbget`.
    pub kind: String,
    pub base_url: String,
    /// NZBGet Basic-auth username; `None` for SABnzbd.
    pub username: Option<String>,
    /// Dual-use: SABnzbd apikey OR NZBGet control password.
    pub secret: String,
    pub category: String,
    pub enabled: bool,
    pub updated_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewDownloaderConfig {
    pub kind: String,
    pub base_url: String,
    pub username: Option<String>,
    pub secret: String,
    pub category: String,
    pub enabled: bool,
}

/// Fetch the configured downloader, or `None` if none is set.
pub async fn get<'e, E>(executor: E) -> Result<Option<DownloaderConfigRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        DownloaderConfigRow,
        r#"SELECT id AS "id!: i64", kind, base_url, username, secret,
                  category, enabled AS "enabled!: bool",
                  updated_at AS "updated_at: _"
           FROM downloader_config WHERE id = 1"#
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Insert-or-replace the single config row (always id = 1).
pub async fn upsert<'e, E>(executor: E, input: NewDownloaderConfig) -> Result<DownloaderConfigRow>
where
    E: SqliteExecutor<'e>,
{
    let enabled = i64::from(input.enabled);
    let row = sqlx::query_as!(
        DownloaderConfigRow,
        r#"INSERT OR REPLACE INTO downloader_config
               (id, kind, base_url, username, secret, category, enabled, updated_at)
           VALUES (1, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
           RETURNING id AS "id!: i64", kind, base_url, username, secret,
                     category, enabled AS "enabled!: bool",
                     updated_at AS "updated_at: _""#,
        input.kind,
        input.base_url,
        input.username,
        input.secret,
        input.category,
        enabled,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Remove the downloader config. No-op if none is set.
pub async fn clear<'e, E>(executor: E) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(r#"DELETE FROM downloader_config WHERE id = 1"#)
        .execute(executor)
        .await?;
    Ok(())
}
