//! Repository for `pull_list` — series subscribed for auto-pull.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct PullListRow {
    pub id: i64,
    pub series_id: i64,
    pub added_at: PrimitiveDateTime,
    /// `None` = pull from the first solicited issue.
    pub start_issue: Option<String>,
    pub paused: bool,
    pub last_pull_attempt_at: Option<PrimitiveDateTime>,
    pub last_successful_pull_at: Option<PrimitiveDateTime>,
    pub failure_count: i64,
}

#[derive(Debug, Clone)]
pub struct NewPullEntry {
    pub series_id: i64,
    pub start_issue: Option<String>,
}

/// Subscribe a series. A second add for the same series hits the
/// `UNIQUE(series_id)` constraint and errors as [`DbError`].
pub async fn add<'e, E>(executor: E, input: NewPullEntry) -> Result<PullListRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        PullListRow,
        r#"INSERT INTO pull_list (series_id, start_issue)
           VALUES (?, ?)
           RETURNING id AS "id!: i64", series_id AS "series_id!: i64",
                     added_at AS "added_at: _", start_issue,
                     paused AS "paused!: bool",
                     last_pull_attempt_at AS "last_pull_attempt_at: _",
                     last_successful_pull_at AS "last_successful_pull_at: _",
                     failure_count AS "failure_count!: i64""#,
        input.series_id,
        input.start_issue,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Unsubscribe a series.
pub async fn remove<'e, E>(executor: E, series_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM pull_list WHERE series_id = ?"#, series_id)
        .execute(executor)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn get<'e, E>(executor: E, series_id: i64) -> Result<Option<PullListRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        PullListRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  added_at AS "added_at: _", start_issue,
                  paused AS "paused!: bool",
                  last_pull_attempt_at AS "last_pull_attempt_at: _",
                  last_successful_pull_at AS "last_successful_pull_at: _",
                  failure_count AS "failure_count!: i64"
           FROM pull_list WHERE series_id = ?"#,
        series_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn list_all<'e, E>(executor: E) -> Result<Vec<PullListRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        PullListRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  added_at AS "added_at: _", start_issue,
                  paused AS "paused!: bool",
                  last_pull_attempt_at AS "last_pull_attempt_at: _",
                  last_successful_pull_at AS "last_successful_pull_at: _",
                  failure_count AS "failure_count!: i64"
           FROM pull_list ORDER BY added_at DESC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Active (non-paused) entries — the pull engine's working set.
pub async fn list_active<'e, E>(executor: E) -> Result<Vec<PullListRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        PullListRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  added_at AS "added_at: _", start_issue,
                  paused AS "paused!: bool",
                  last_pull_attempt_at AS "last_pull_attempt_at: _",
                  last_successful_pull_at AS "last_successful_pull_at: _",
                  failure_count AS "failure_count!: i64"
           FROM pull_list WHERE paused = 0 ORDER BY added_at ASC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn set_paused<'e, E>(executor: E, series_id: i64, paused: bool) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let paused = i64::from(paused);
    let result = sqlx::query!(
        r#"UPDATE pull_list SET paused = ? WHERE series_id = ?"#,
        paused,
        series_id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Record a failed pull sweep for a series: stamps `last_pull_attempt_at`
/// and increments `failure_count`.
pub async fn mark_attempt_failed<'e, E>(executor: E, series_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_list
           SET last_pull_attempt_at = CURRENT_TIMESTAMP,
               failure_count = failure_count + 1
           WHERE series_id = ?"#,
        series_id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Record a successful pull: stamps both timestamps and resets
/// `failure_count` to 0.
pub async fn mark_attempt_succeeded<'e, E>(executor: E, series_id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_list
           SET last_pull_attempt_at = CURRENT_TIMESTAMP,
               last_successful_pull_at = CURRENT_TIMESTAMP,
               failure_count = 0
           WHERE series_id = ?"#,
        series_id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}
