use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, Deserialize)]
pub struct ParsingPatternRow {
    pub id: i64,
    pub name: String,
    pub pattern: String,
    pub priority: i64,
    /// Stored as INTEGER (0/1); decoded as bool via sqlx-sqlite.
    pub enabled: bool,
    pub created_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewParsingPattern {
    pub name: String,
    pub pattern: String,
    pub priority: i32,
    pub enabled: bool,
}

pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<ParsingPatternRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        ParsingPatternRow,
        r#"SELECT id AS "id!: i64", name, pattern,
                  priority AS "priority!: i64",
                  enabled AS "enabled!: bool",
                  created_at AS "created_at: _"
           FROM parsing_patterns WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Returns enabled patterns in priority order (ascending — lower number runs
/// first). This is what the scanner loads to feed the matcher.
pub async fn list_enabled<'e, E>(executor: E) -> Result<Vec<ParsingPatternRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        ParsingPatternRow,
        r#"SELECT id AS "id!: i64", name, pattern,
                  priority AS "priority!: i64",
                  enabled AS "enabled!: bool",
                  created_at AS "created_at: _"
           FROM parsing_patterns
           WHERE enabled = 1
           ORDER BY priority, id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn list_all<'e, E>(executor: E) -> Result<Vec<ParsingPatternRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        ParsingPatternRow,
        r#"SELECT id AS "id!: i64", name, pattern,
                  priority AS "priority!: i64",
                  enabled AS "enabled!: bool",
                  created_at AS "created_at: _"
           FROM parsing_patterns
           ORDER BY priority, id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn insert<'e, E>(executor: E, input: NewParsingPattern) -> Result<ParsingPatternRow>
where
    E: SqliteExecutor<'e>,
{
    let priority = i64::from(input.priority);
    let enabled = if input.enabled { 1_i64 } else { 0_i64 };
    let row = sqlx::query_as!(
        ParsingPatternRow,
        r#"INSERT INTO parsing_patterns (name, pattern, priority, enabled)
           VALUES (?, ?, ?, ?)
           RETURNING id AS "id!: i64", name, pattern,
                     priority AS "priority!: i64",
                     enabled AS "enabled!: bool",
                     created_at AS "created_at: _""#,
        input.name,
        input.pattern,
        priority,
        enabled
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(
    executor: E,
    id: i64,
    input: NewParsingPattern,
) -> Result<ParsingPatternRow>
where
    E: SqliteExecutor<'e>,
{
    let priority = i64::from(input.priority);
    let enabled = if input.enabled { 1_i64 } else { 0_i64 };
    let row = sqlx::query_as!(
        ParsingPatternRow,
        r#"UPDATE parsing_patterns
           SET name = ?, pattern = ?, priority = ?, enabled = ?
           WHERE id = ?
           RETURNING id AS "id!: i64", name, pattern,
                     priority AS "priority!: i64",
                     enabled AS "enabled!: bool",
                     created_at AS "created_at: _""#,
        input.name,
        input.pattern,
        priority,
        enabled,
        id
    )
    .fetch_optional(executor)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(row)
}

pub async fn set_enabled<'e, E>(executor: E, id: i64, enabled: bool) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let value = if enabled { 1_i64 } else { 0_i64 };
    let result = sqlx::query!(
        r#"UPDATE parsing_patterns SET enabled = ? WHERE id = ?"#,
        value,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}
