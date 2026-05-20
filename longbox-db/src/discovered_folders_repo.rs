//! Repository for `discovered_folders` — top-level library subfolders
//! that look series-shaped but don't resolve to any tracked series.
//!
//! The scanner upserts detections here; the reconciliation view lists
//! the non-dismissed ones for the user to add or dismiss.

use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqliteExecutor};
use time::PrimitiveDateTime;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct DiscoveredFolderRow {
    pub id: i64,
    pub folder_name: String,
    pub first_seen_at: PrimitiveDateTime,
    pub last_seen_at: PrimitiveDateTime,
    /// Set when the user opts out of tracking this folder; `None`
    /// while it is still an open discovery.
    pub dismissed_at: Option<PrimitiveDateTime>,
    pub file_count: i64,
}

/// Scanner input for [`upsert`].
#[derive(Debug, Clone)]
pub struct DiscoveredFolder {
    pub folder_name: String,
    pub file_count: i64,
}

/// Record a detected folder: insert it if new, or refresh
/// `last_seen_at` + `file_count` if already present. A *dismissed*
/// folder is left untouched — the `WHERE dismissed_at IS NULL` guard
/// on the conflict-update makes re-detection a clean no-op, so a
/// folder the user opted out of is never resurrected.
pub async fn upsert<'e, E>(executor: E, input: DiscoveredFolder) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO discovered_folders (folder_name, file_count)
           VALUES (?, ?)
           ON CONFLICT(folder_name) DO UPDATE SET
               last_seen_at = CURRENT_TIMESTAMP,
               file_count = excluded.file_count
           WHERE dismissed_at IS NULL"#,
        input.folder_name,
        input.file_count,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Non-dismissed discovered folders, oldest discovery first — the
/// reconciliation view's working set.
pub async fn list<'e, E>(executor: E) -> Result<Vec<DiscoveredFolderRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        DiscoveredFolderRow,
        r#"SELECT id AS "id!: i64", folder_name,
                  first_seen_at AS "first_seen_at: _",
                  last_seen_at AS "last_seen_at: _",
                  dismissed_at AS "dismissed_at: _",
                  file_count AS "file_count!: i64"
           FROM discovered_folders
           WHERE dismissed_at IS NULL
           ORDER BY first_seen_at ASC, folder_name ASC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Mark a set of folders dismissed. Idempotent — an already-dismissed
/// folder is left as-is (the `dismissed_at IS NULL` guard). Returns the
/// count of rows newly dismissed.
pub async fn dismiss<'e, E>(executor: E, folder_names: &[String]) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    if folder_names.is_empty() {
        return Ok(0);
    }
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "UPDATE discovered_folders SET dismissed_at = CURRENT_TIMESTAMP \
         WHERE dismissed_at IS NULL AND folder_name IN (",
    );
    let mut separated = qb.separated(", ");
    for name in folder_names {
        separated.push_bind(name);
    }
    separated.push_unseparated(")");
    let result = qb.build().execute(executor).await?;
    Ok(result.rows_affected())
}
