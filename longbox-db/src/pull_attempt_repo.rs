//! Repository for `pull_attempts` — the per-attempt audit +
//! retry-exclusion log.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct PullAttemptRow {
    pub id: i64,
    pub series_id: i64,
    pub issue_id: i64,
    pub attempted_at: PrimitiveDateTime,
    /// `None` once the indexer that served the release is deconfigured.
    pub indexer_id: Option<i64>,
    /// The Newznab release guid — used for retry-exclusion (don't
    /// re-grab a release that already failed).
    pub release_id: Option<String>,
    /// `pending` | `submitted` | `grabbed` | `failed` | `mismatched`.
    pub status: String,
    pub error_message: Option<String>,
    pub retry_count: i64,
}

#[derive(Debug, Clone)]
pub struct NewPullAttempt {
    pub series_id: i64,
    pub issue_id: i64,
    pub indexer_id: Option<i64>,
    pub release_id: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub retry_count: i64,
}

pub async fn insert<'e, E>(executor: E, input: NewPullAttempt) -> Result<PullAttemptRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        PullAttemptRow,
        r#"INSERT INTO pull_attempts
               (series_id, issue_id, indexer_id, release_id, status,
                error_message, retry_count)
           VALUES (?, ?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", series_id AS "series_id!: i64",
                     issue_id AS "issue_id!: i64", attempted_at AS "attempted_at: _",
                     indexer_id, release_id, status, error_message,
                     retry_count AS "retry_count!: i64""#,
        input.series_id,
        input.issue_id,
        input.indexer_id,
        input.release_id,
        input.status,
        input.error_message,
        input.retry_count,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// All attempts for an issue, newest first — audit view + the source
/// of already-tried `release_id`s for retry exclusion.
pub async fn list_for_issue<'e, E>(
    executor: E,
    series_id: i64,
    issue_id: i64,
) -> Result<Vec<PullAttemptRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        PullAttemptRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  issue_id AS "issue_id!: i64", attempted_at AS "attempted_at: _",
                  indexer_id, release_id, status, error_message,
                  retry_count AS "retry_count!: i64"
           FROM pull_attempts
           WHERE series_id = ? AND issue_id = ?
           ORDER BY attempted_at DESC, id DESC"#,
        series_id,
        issue_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Whether an issue has an in-flight attempt (`pending` or
/// `submitted`). Phase B's processor calls this to decide whether a
/// caught file should be attributed to the pull engine.
pub async fn has_in_flight_attempt<'e, E>(
    executor: E,
    series_id: i64,
    issue_id: i64,
) -> Result<bool>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM pull_attempts
           WHERE series_id = ? AND issue_id = ?
             AND status IN ('pending', 'submitted')"#,
        series_id,
        issue_id
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count > 0)
}

/// Transition every in-flight (`pending`/`submitted`) attempt for an
/// issue to `grabbed`. Phase B calls this when it catches a file —
/// the multi-row update is deliberate: 2+ in-flight attempts for the
/// same issue (a race) all settle to `grabbed`. Returns the count
/// transitioned.
pub async fn mark_grabbed_for_issue<'e, E>(
    executor: E,
    series_id: i64,
    issue_id: i64,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_attempts
           SET status = 'grabbed'
           WHERE series_id = ? AND issue_id = ?
             AND status IN ('pending', 'submitted')"#,
        series_id,
        issue_id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Update one attempt's status (+ optional error message). Used by the
/// pull engine to record submission / failure transitions.
pub async fn update_status<'e, E>(
    executor: E,
    id: i64,
    status: &str,
    error_message: Option<&str>,
) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_attempts SET status = ?, error_message = ? WHERE id = ?"#,
        status,
        error_message,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(crate::error::DbError::NotFound);
    }
    Ok(())
}
