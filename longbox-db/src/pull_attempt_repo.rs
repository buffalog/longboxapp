//! Repository for `pull_attempts` — the per-attempt audit +
//! retry-exclusion log.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

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
    /// Cumulative failed attempts for this issue, counting this row
    /// when its status is `failed`. The pull engine parks an issue
    /// (stops generating candidates) once any attempt reaches 3.
    pub retry_count: i64,
    /// Downloader job id (SABnzbd nzo_id / NZBGet NZBID), captured at
    /// submission. `None` until a submit succeeds.
    pub download_handle: Option<String>,
    /// Consecutive status polls that returned `Unknown`.
    pub unknown_polls: i64,
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
    pub download_handle: Option<String>,
}

pub async fn insert<'e, E>(executor: E, input: NewPullAttempt) -> Result<PullAttemptRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        PullAttemptRow,
        r#"INSERT INTO pull_attempts
               (series_id, issue_id, indexer_id, release_id, status,
                error_message, retry_count, download_handle)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", series_id AS "series_id!: i64",
                     issue_id AS "issue_id!: i64", attempted_at AS "attempted_at: _",
                     indexer_id, release_id, status, error_message,
                     retry_count AS "retry_count!: i64", download_handle,
                     unknown_polls AS "unknown_polls!: i64""#,
        input.series_id,
        input.issue_id,
        input.indexer_id,
        input.release_id,
        input.status,
        input.error_message,
        input.retry_count,
        input.download_handle,
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
                  retry_count AS "retry_count!: i64", download_handle,
                  unknown_polls AS "unknown_polls!: i64"
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

/// A pull failure for the needs-attention surface — the latest attempt
/// for an issue, when that attempt is `failed` or `mismatched`, joined
/// with the display fields a `pull_attempts` row lacks. One row per
/// issue.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct FailedPull {
    pub series_id: i64,
    pub issue_id: i64,
    pub series_title: String,
    pub issue_number: String,
    /// `None` for a submission failure or a series mismatch (no release
    /// ever landed); `Some` for a grab failure (a submitted release that
    /// then failed). Together with `status` lets the route handler
    /// categorize into submission_failed / grab_failed / series_mismatch.
    pub release_id: Option<String>,
    /// `'failed'` or `'mismatched'` — the route handler routes on this
    /// alongside `release_id` to derive the user-facing category.
    pub status: String,
    pub error_message: Option<String>,
    pub retry_count: i64,
    pub attempted_at: PrimitiveDateTime,
}

/// Issues whose most-recent pull attempt is in a failure-class state
/// (`'failed'` or `'mismatched'`) — the pull side of the needs-attention
/// surface. One row per issue (its latest attempt); an issue retried
/// since (latest attempt `submitted`/`grabbed`) drops off. Bug 3 added
/// `'mismatched'` to the listed set so series-mismatch outcomes surface
/// the same way submission/grab failures do.
pub async fn list_failed<'e, E>(executor: E) -> Result<Vec<FailedPull>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        FailedPull,
        r#"SELECT pa.series_id AS "series_id!: i64", pa.issue_id AS "issue_id!: i64",
                  s.title AS "series_title!: String", i.number AS "issue_number!: String",
                  pa.release_id,
                  pa.status AS "status!: String",
                  pa.error_message,
                  pa.retry_count AS "retry_count!: i64",
                  pa.attempted_at AS "attempted_at: _"
           FROM pull_attempts pa
           JOIN series s ON pa.series_id = s.id
           JOIN issues i ON pa.issue_id = i.id
           WHERE pa.status IN ('failed', 'mismatched')
             AND pa.id = (
                 SELECT MAX(p2.id) FROM pull_attempts p2
                 WHERE p2.series_id = pa.series_id AND p2.issue_id = pa.issue_id
             )
           ORDER BY pa.attempted_at DESC, pa.id DESC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Delete an issue's failure-class pull attempts (`'failed'` and
/// `'mismatched'`) — the "Retry" un-park. With the failure history gone
/// the candidate query no longer treats the issue as parked, so the
/// next sweep attempts it fresh. Returns the number of rows cleared.
pub async fn clear_failed_for_issue<'e, E>(
    executor: E,
    series_id: i64,
    issue_id: i64,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"DELETE FROM pull_attempts
           WHERE series_id = ?
             AND issue_id = ?
             AND status IN ('failed', 'mismatched')"#,
        series_id,
        issue_id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Record a Bug 3 series-title mismatch — no release survived the pull
/// engine's pre-grab newznab filter. Inserts a fresh `pull_attempts`
/// row with `status='mismatched'`, `release_id=NULL`,
/// `download_handle=NULL`, mirroring the engine's submission-failure
/// shape so that 3 mismatches in a row park the issue via the candidate
/// query's `retry_count >= 3` check.
///
/// `prior_failure_count` is the running cumulative count the engine
/// already maintains for the issue (it's the same value passed to a
/// submission-failure insert). The new row's `retry_count` is
/// `prior_failure_count + 1`.
pub async fn record_mismatch<'e, E>(
    executor: E,
    series_id: i64,
    issue_id: i64,
    indexer_id: Option<i64>,
    error_message: &str,
    prior_failure_count: i64,
) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let new_retry = prior_failure_count + 1;
    let row = sqlx::query!(
        r#"INSERT INTO pull_attempts
               (series_id, issue_id, indexer_id, release_id, status,
                error_message, retry_count, download_handle, unknown_polls)
           VALUES (?, ?, ?, NULL, 'mismatched', ?, ?, NULL, 0)
           RETURNING id AS "id!: i64""#,
        series_id,
        issue_id,
        indexer_id,
        error_message,
        new_retry,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.id)
}

/// Every `submitted` attempt — the pull engine's in-flight set, polled
/// for downloader status at the top of each sweep.
pub async fn list_submitted<'e, E>(executor: E) -> Result<Vec<PullAttemptRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        PullAttemptRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  issue_id AS "issue_id!: i64", attempted_at AS "attempted_at: _",
                  indexer_id, release_id, status, error_message,
                  retry_count AS "retry_count!: i64", download_handle,
                  unknown_polls AS "unknown_polls!: i64"
           FROM pull_attempts
           WHERE status = 'submitted'
           ORDER BY attempted_at ASC, id ASC"#
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

/// Update one attempt's status (+ optional error message). Used for
/// transitions that don't change the retry count.
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
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Transition an attempt to `failed` and bump its `retry_count` — the
/// engine calls this on a submission failure, a downloader grab
/// failure, or a lost-track timeout. The bumped `retry_count` is what
/// the candidate query checks against the parking threshold (3).
pub async fn record_failure<'e, E>(executor: E, id: i64, error_message: &str) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_attempts
           SET status = 'failed',
               error_message = ?,
               retry_count = retry_count + 1
           WHERE id = ?"#,
        error_message,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Increment an attempt's consecutive-`Unknown` poll counter. The
/// engine calls this when a downloader status poll returns `Unknown`
/// but the threshold for giving up hasn't been reached.
pub async fn bump_unknown_polls<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_attempts SET unknown_polls = unknown_polls + 1 WHERE id = ?"#,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Reset an attempt's `Unknown`-poll counter to zero — called when a
/// poll returns a *known* status again, keeping the counter a measure
/// of strictly *consecutive* Unknowns.
pub async fn reset_unknown_polls<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_attempts SET unknown_polls = 0 WHERE id = ?"#,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}
