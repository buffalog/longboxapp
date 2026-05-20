use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

/// Curated default blocklist. Names verified against ComicVine's actual
/// publisher field. `reset-to-defaults` (re-)inserts any of these that
/// aren't already present; user-added rows are preserved.
pub const DEFAULT_BLOCKED_PUBLISHERS: &[&str] = &[
    "Panini Comics",
    "Panini France",
    "Panini Brasil",
    "Planeta DeAgostini",
    "Editorial Televisa",
    "ECC Ediciones",
    "Norma Editorial",
    "Éditions Glénat",
    "Arnoldo Mondadori Editore",
    "Salvat",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherFilterMode {
    Block,
    Allow,
}

impl PublisherFilterMode {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Allow => "allow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct PublisherFilterRow {
    pub id: i64,
    pub publisher_name: String,
    /// Enum as TEXT; `block` | `allow`.
    pub mode: String,
    pub created_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewPublisherFilter {
    pub publisher_name: String,
    pub mode: PublisherFilterMode,
}

pub async fn list_all<'e, E>(executor: E) -> Result<Vec<PublisherFilterRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        PublisherFilterRow,
        r#"SELECT id AS "id!: i64", publisher_name, mode,
                  created_at AS "created_at: _"
           FROM publisher_filters
           ORDER BY publisher_name COLLATE NOCASE"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Convenience: just the publisher names of `block` rows, lowercased
/// for direct comparison against CV's publisher strings. CV search uses
/// this on every request.
pub async fn blocked_names_lower<'e, E>(executor: E) -> Result<Vec<String>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(r#"SELECT publisher_name FROM publisher_filters WHERE mode = 'block'"#)
        .fetch_all(executor)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.publisher_name.to_lowercase())
        .collect())
}

pub async fn insert<'e, E>(executor: E, input: NewPublisherFilter) -> Result<PublisherFilterRow>
where
    E: SqliteExecutor<'e>,
{
    let mode = input.mode.as_db_str();
    let row = sqlx::query_as!(
        PublisherFilterRow,
        r#"INSERT INTO publisher_filters (publisher_name, mode)
           VALUES (?, ?)
           RETURNING id AS "id!: i64", publisher_name, mode,
                     created_at AS "created_at: _""#,
        input.publisher_name,
        mode,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn delete<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM publisher_filters WHERE id = ?"#, id)
        .execute(executor)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Inserts any of [`DEFAULT_BLOCKED_PUBLISHERS`] that aren't already
/// present, leaving user-added rows untouched. Returns the count of
/// newly-inserted rows.
///
/// Takes a `&Pool` directly rather than the usual generic executor —
/// the operation loops and each iteration needs its own executor borrow.
/// Reset is an operational one-shot, never composed into a transaction.
pub async fn reset_to_defaults(pool: &crate::Pool) -> Result<u64> {
    let mut inserted: u64 = 0;
    // SQLite's INSERT OR IGNORE relies on the UNIQUE constraint to skip
    // duplicates; COLLATE NOCASE on the column makes "panini comics" and
    // "Panini Comics" collide as expected.
    for name in DEFAULT_BLOCKED_PUBLISHERS {
        let result = sqlx::query!(
            r#"INSERT OR IGNORE INTO publisher_filters (publisher_name, mode)
               VALUES (?, 'block')"#,
            name
        )
        .execute(pool)
        .await?;
        inserted += result.rows_affected();
    }
    Ok(inserted)
}
