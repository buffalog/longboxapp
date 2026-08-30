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
    /// `pull_attempts.id` — the surface-row key the dismiss endpoint
    /// uses to delete this specific attempt. Distinct from
    /// `(series_id, issue_id)` because a single issue can have several
    /// failure-class attempts on file; `list_failed` only surfaces the
    /// latest one but each row carries its own attempts-table id.
    pub id: i64,
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
        r#"SELECT pa.id AS "id!: i64",
                  pa.series_id AS "series_id!: i64", pa.issue_id AS "issue_id!: i64",
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

/// Delete a single `pull_attempts` row by id. Powers the per-row
/// "Dismiss" action on the needs-attention surface. Returns `true` if a
/// row was deleted, `false` if no row matched (stale UI / double-click).
/// Surgical by design — if older failure-class attempts exist for the
/// same issue, the next list_failed pulls the previous one as the new
/// surface row, and the user can dismiss again or use bulk-clear.
pub async fn delete_by_id<'e, E>(executor: E, id: i64) -> Result<bool>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM pull_attempts WHERE id = ?"#, id)
        .execute(executor)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete every failure-class `pull_attempts` row — the "Clear all"
/// action on the needs-attention surface. Status filter mirrors
/// [`list_failed`] so we only remove what was visible. Returns the
/// number of rows deleted.
pub async fn delete_all_failed<'e, E>(executor: E) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result =
        sqlx::query!(r#"DELETE FROM pull_attempts WHERE status IN ('failed', 'mismatched')"#)
            .execute(executor)
            .await?;
    Ok(result.rows_affected())
}

/// Single-row lookup for the dismiss endpoint. Returns `None` for a
/// stale-UI / double-click delete on an already-gone row. Only the
/// fields the dismiss flow needs (series_id, issue_id) are extracted —
/// callers chasing the full row should use [`list_for_issue`].
pub async fn find_series_issue_by_id<'e, E>(executor: E, id: i64) -> Result<Option<(i64, i64)>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT series_id AS "series_id!: i64", issue_id AS "issue_id!: i64"
           FROM pull_attempts WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| (r.series_id, r.issue_id)))
}

/// Age threshold for the "stale submitted" purge. An attempt that's
/// been `submitted` longer than this is presumed dropped — the poll
/// loop lost track (container restart orphaned the SAB job handle, SAB
/// evicted it from its queue, indexer hiccup wedged the row). 6h is
/// well past any reasonable SAB download time; tune up if real
/// large-queue SAB deployments surface false purges.
///
/// Kept as a documented constant in Rust + hardcoded as
/// `datetime('now', '-6 hours')` in the SQL bodies below — sqlx's
/// compile-time check needs SQL literals, so the two MUST stay in
/// lockstep when changed.
pub const STALE_SUBMITTED_HOURS: i64 = 6;

/// Delete `submitted` pull attempts for one issue whose `attempted_at`
/// is older than [`STALE_SUBMITTED_HOURS`]. Returns the number of rows
/// removed. Called from the on-demand search path
/// (`engine::sweep_single_issue`) and from the `dismiss_pull_failure`
/// endpoint so a stale row never blocks a legitimate retry. Genuinely
/// fresh `submitted` rows (poll loop hasn't caught up yet) are left
/// alone — they're potentially still being downloaded.
pub async fn purge_stale_submitted_for_issue<'e, E>(
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
             AND status = 'submitted'
             AND attempted_at < datetime('now', '-6 hours')"#,
        series_id,
        issue_id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Age threshold for the "stale grabbed" purge. A `grabbed` row that's
/// stuck this long without a matching owned+present file is presumed
/// dead — Phase B never saw the import (SAB silently failed, the file
/// got cleaned out of `/watch/`, the source archive was malformed and
/// rejected, etc.). 24h is well past any reasonable SAB+Phase-B
/// pipeline; tune up if real deployments surface false purges.
///
/// `grabbed` rows that DO have an owned+present file are audit history
/// of a successful pull — those are never purged regardless of age.
///
/// Kept as a documented constant in Rust + hardcoded as
/// `datetime('now', '-24 hours')` in the SQL bodies below — sqlx's
/// compile-time check needs SQL literals, so the two MUST stay in
/// lockstep when changed.
pub const STALE_GRABBED_HOURS: i64 = 24;

/// Delete `grabbed` pull attempts for one issue older than
/// [`STALE_GRABBED_HOURS`] **only when the issue has no owned+present
/// file**. The owned-file guard preserves the audit trail of
/// successful pulls (the row's `release_id` feeds the exclusion list
/// on future re-pulls). Returns the number of rows removed. Called
/// from the on-demand search path (`engine::sweep_single_issue`) and
/// from `dismiss_pull_failure` so a stuck-grabbed row never blocks a
/// legitimate retry.
pub async fn purge_stale_grabbed_for_issue<'e, E>(
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
             AND status = 'grabbed'
             AND attempted_at < datetime('now', '-24 hours')
             AND NOT EXISTS (
               SELECT 1 FROM issue_ownership o
               WHERE o.issue_id = pull_attempts.issue_id AND o.is_owned = 1
             )"#,
        series_id,
        issue_id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Workspace-wide counterpart to [`purge_stale_grabbed_for_issue`].
/// One DELETE across the whole DB — the scheduled `sweep()` calls
/// this once at the top so issues whose Phase B import never landed
/// re-enter `list_pull_candidates`. Same owned-file guard preserves
/// successful-pull audit rows. Returns the number of rows removed.
pub async fn purge_stale_grabbed_workspace<'e, E>(executor: E) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"DELETE FROM pull_attempts
           WHERE status = 'grabbed'
             AND attempted_at < datetime('now', '-24 hours')
             AND NOT EXISTS (
               SELECT 1 FROM issue_ownership o
               WHERE o.issue_id = pull_attempts.issue_id AND o.is_owned = 1
             )"#
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
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

/// Find a `submitted` pull attempt by its downloader job handle
/// (`nzo_id` for SABnzbd, `NZBID` for NZBGet). Returns `None` when the
/// handle isn't found or the attempt is not in `submitted` state
/// (already failed, grabbed, etc.). Used by the
/// `POST /downloader/notify` webhook so a SAB failure notification
/// fires immediate retry instead of waiting for the next sweep poll.
pub async fn find_submitted_by_handle<'e, E>(
    executor: E,
    handle: &str,
) -> Result<Option<PullAttemptRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        PullAttemptRow,
        r#"SELECT id AS "id!: i64", series_id AS "series_id!: i64",
                  issue_id AS "issue_id!: i64", attempted_at AS "attempted_at: _",
                  indexer_id, release_id, status, error_message,
                  retry_count AS "retry_count!: i64", download_handle,
                  unknown_polls AS "unknown_polls!: i64"
           FROM pull_attempts
           WHERE download_handle = ? AND status = 'submitted'"#,
        handle
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
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

/// Whether the pull engine has *ever* recorded an attempt for this
/// issue — any status, any age. Phase B uses this as a confidence
/// override: when a caught file's matcher score is sub-threshold but
/// the issue carries pull-attempt history, the pull engine already
/// verified the series at indexer time (the `q`-term and similarity
/// filter cleared the title before the NZB was even submitted), so
/// Phase B trusts the match regardless of the local score.
///
/// Catches the recurring "scene punctuation/caps don't match the
/// catalog title verbatim" case — "Y The Last Man" vs "Y: The Last
/// Man" landing at 0.71 confidence; "Hell to Pay" vs "Hell To Pay"
/// at 0.73; "DC K.O. - Superman vs. Captain Atom" at 0.67. All
/// correct matches, all rejected by the 0.75 floor, all originally
/// requested by the pull engine.
///
/// Distinct from [`has_in_flight_attempt`] (which is narrower:
/// `pending|submitted` only, used for grab/import attribution).
pub async fn issue_has_attempt<'e, E>(executor: E, issue_id: i64) -> Result<bool>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT 1 AS "exists!: i64"
           FROM pull_attempts
           WHERE issue_id = ?
           LIMIT 1"#,
        issue_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

/// The attempt-level reasons the pull sweep would skip this issue.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AttemptBlockers {
    /// Status of a surviving `pending`/`grabbed` attempt, if any.
    pub in_flight_status: Option<String>,
    /// Some attempt has `retry_count >= 3`.
    pub retries_exhausted: bool,
    /// A `submitted` attempt landed inside the 6-hour window.
    pub recently_submitted: bool,
}

/// The three attempt-level exclusions `list_pull_candidates` applies,
/// reported individually.
///
/// **Facts, not a candidacy derivation.** Each field mirrors exactly
/// one disjunct of that query's `NOT EXISTS`, and the caller decides
/// what to say about them. Reimplementing the candidate query here
/// instead would be a second implementation of one rule, which is how
/// the two drift — and this one only has to explain a user-visible
/// outcome, never to decide one.
///
/// Why all three rather than just the in-flight status this used to
/// return: an issue that has reverted to missing and will NOT be
/// re-searched looks identical, from the outside, to one that will.
/// Saying "reverts to missing" and stopping there invites the user to
/// wait for a sweep that is never going to look at it. Two of these
/// fire zero times on the current library; they are encoded rather
/// than assumed precisely because "zero today" is not "zero".
///
/// This does NOT cover the series-level exclusions — pull-list
/// membership, `paused`, the `start_issue` floor — or the issue's own
/// `cover_date`. Those live on rows the caller already holds.
pub async fn attempt_blockers<'e, E>(executor: E, issue_id: i64) -> Result<AttemptBlockers>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT
             (SELECT status FROM pull_attempts
               WHERE issue_id = ?1 AND status IN ('pending', 'grabbed')
               ORDER BY attempted_at DESC LIMIT 1) AS "in_flight_status?: String",
             EXISTS(SELECT 1 FROM pull_attempts
                     WHERE issue_id = ?1 AND retry_count >= 3) AS "retries_exhausted!: bool",
             EXISTS(SELECT 1 FROM pull_attempts
                     WHERE issue_id = ?1 AND status = 'submitted'
                       AND attempted_at >= datetime('now', '-6 hours'))
               AS "recently_submitted!: bool""#,
        issue_id
    )
    .fetch_one(executor)
    .await?;
    Ok(AttemptBlockers {
        in_flight_status: row.in_flight_status,
        retries_exhausted: row.retries_exhausted,
        recently_submitted: row.recently_submitted,
    })
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

/// Transition every in-flight (`pending`/`submitted`) attempt for an
/// issue to `failed`, recording why. The mirror of
/// [`mark_grabbed_for_issue`], for the case where the file arrived and
/// turned out to be unusable.
///
/// Phase B is the only component that can reach this verdict. Comic
/// NZBs routinely carry no par2 recovery set, so the downloader has no
/// checksums, reports the job Completed in good faith, and the
/// corruption inside the archive is invisible until something opens it.
/// Without this the attempt sat `submitted` until three polls of
/// `Unknown` aged it out as "lost track of download" — which is not
/// what happened and tells the user nothing.
///
/// `retry_count` is bumped, matching [`record_failure_if_submitted`]:
/// this was a real consumed attempt, not a no-op.
pub async fn record_failure_for_issue<'e, E>(
    executor: E,
    series_id: i64,
    issue_id: i64,
    error_message: &str,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE pull_attempts
           SET status = 'failed',
               error_message = ?,
               retry_count = retry_count + 1
           WHERE series_id = ? AND issue_id = ?
             AND status IN ('pending', 'submitted')"#,
        error_message,
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

/// Transition an attempt to `failed` and bump its `retry_count`, **but
/// only if the row is still `submitted`**. The engine calls this on a
/// submission failure, a downloader grab failure, or a lost-track
/// timeout. The bumped `retry_count` is what the candidate query
/// checks against the parking threshold (3).
///
/// # Why the predicate
///
/// The poll loop snapshots rows with `list_submitted`, then does I/O
/// (a downloader round-trip, and for the landed-content check a
/// filesystem walk), then writes. Phase B can import the comic and
/// call `mark_grabbed_for_issue` inside that window — and that
/// function is correctly scoped to `('pending','submitted')`, so
/// without the same predicate here the poll path could overwrite a
/// `grabbed` row that had already moved on. An unscoped variant of
/// this write existed and was deleted rather than left available:
/// there is no caller for which overwriting a settled row is correct.
///
/// A row that has moved on is not ours to overwrite. The predicate
/// makes the write a no-op in exactly that case.
///
/// Correctness here is argued from the predicate, not demonstrated by
/// the suite: the test harness is in-memory SQLite at
/// `max_connections = 1` and structurally cannot produce the
/// interleaving this guards against.
pub async fn record_failure_if_submitted<'e, E>(
    executor: E,
    id: i64,
    error_message: &str,
) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"UPDATE pull_attempts
           SET status = 'failed',
               error_message = ?,
               retry_count = retry_count + 1
           WHERE id = ? AND status = 'submitted'"#,
        error_message,
        id
    )
    .execute(executor)
    .await?;
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
