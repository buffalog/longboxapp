use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanRunStatus {
    Running,
    Completed,
    Failed,
}

impl ScanRunStatus {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanRunKind {
    Full,
    RescanUnmatched,
    RematchForSeries,
}

impl ScanRunKind {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::RescanUnmatched => "rescan_unmatched",
            Self::RematchForSeries => "rematch_for_series",
        }
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct ScanRunRow {
    pub id: i64,
    pub library_root_id: i64,
    pub started_at: PrimitiveDateTime,
    pub finished_at: Option<PrimitiveDateTime>,
    pub files_seen: i64,
    pub files_added: i64,
    pub files_updated: i64,
    pub files_matched: i64,
    pub files_needs_review: i64,
    pub files_unmatched: i64,
    /// Enum stored as TEXT; convert with [`ScanRunStatus::from_db_str`].
    pub status: String,
    pub error_message: Option<String>,
    /// Enum stored as TEXT (`full` | `rescan_unmatched` | `rematch_for_series`).
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct NewScanRun {
    pub library_root_id: i64,
    pub kind: ScanRunKind,
}

/// Final counter + status block written when a scan completes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanCompletion {
    pub files_seen: i64,
    pub files_added: i64,
    pub files_updated: i64,
    pub files_matched: i64,
    pub files_needs_review: i64,
    pub files_unmatched: i64,
}

/// Counter snapshot pushed by the scanner during a run. All values are
/// absolute; `update_progress` is a single UPDATE, no read-modify-write.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanProgress {
    pub files_seen: i64,
    pub files_added: i64,
    pub files_updated: i64,
    pub files_matched: i64,
    pub files_needs_review: i64,
    pub files_unmatched: i64,
}

pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<ScanRunRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        ScanRunRow,
        r#"SELECT id AS "id!: i64", library_root_id AS "library_root_id!: i64",
                  started_at AS "started_at: _", finished_at AS "finished_at: _",
                  files_seen AS "files_seen!: i64",
                  files_added AS "files_added!: i64",
                  files_updated AS "files_updated!: i64",
                  files_matched AS "files_matched!: i64",
                  files_needs_review AS "files_needs_review!: i64",
                  files_unmatched AS "files_unmatched!: i64",
                  status, error_message, kind
           FROM scan_runs WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Newest-first list of scan_runs, optionally restricted to a kind set.
/// Pass an empty slice to include every kind. Phase A's `/api/scans/recent`
/// excludes `rematch_for_series` to keep auto-rematch noise out of the
/// dashboard.
pub async fn list_recent<'e, E>(
    executor: E,
    limit: u32,
    kinds: &[ScanRunKind],
) -> Result<Vec<ScanRunRow>>
where
    E: SqliteExecutor<'e>,
{
    let limit_i64 = i64::from(limit);
    // sqlx 0.7 + sqlite can't bind a slice; build a CSV of literal kind
    // strings. The strings come from a Rust enum, not user input, so this
    // is injection-safe.
    let kind_filter: Option<String> = if kinds.is_empty() {
        None
    } else {
        let csv = kinds
            .iter()
            .map(|k| format!("'{}'", k.as_db_str()))
            .collect::<Vec<_>>()
            .join(",");
        Some(csv)
    };
    let where_kind = kind_filter
        .as_deref()
        .map(|csv| format!(" AND kind IN ({csv})"))
        .unwrap_or_default();
    // Build the full SQL once. `query_as_unchecked` because the IN-list
    // is interpolated; the rest of the binds are parameters.
    let sql = format!(
        r#"SELECT id, library_root_id,
                  started_at, finished_at,
                  files_seen, files_added, files_updated,
                  files_matched, files_needs_review, files_unmatched,
                  status, error_message, kind
           FROM scan_runs WHERE 1=1{where_kind}
           ORDER BY started_at DESC, id DESC LIMIT ?"#
    );
    let rows = sqlx::query_as::<_, ScanRunRow>(&sql)
        .bind(limit_i64)
        .fetch_all(executor)
        .await?;
    Ok(rows)
}

pub async fn insert<'e, E>(executor: E, input: NewScanRun) -> Result<ScanRunRow>
where
    E: SqliteExecutor<'e>,
{
    let kind = input.kind.as_db_str();
    let row = sqlx::query_as!(
        ScanRunRow,
        r#"INSERT INTO scan_runs (library_root_id, kind) VALUES (?, ?)
           RETURNING id AS "id!: i64", library_root_id AS "library_root_id!: i64",
                     started_at AS "started_at: _", finished_at AS "finished_at: _",
                     files_seen AS "files_seen!: i64",
                     files_added AS "files_added!: i64",
                     files_updated AS "files_updated!: i64",
                     files_matched AS "files_matched!: i64",
                     files_needs_review AS "files_needs_review!: i64",
                     files_unmatched AS "files_unmatched!: i64",
                     status, error_message, kind"#,
        input.library_root_id,
        kind,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Single UPDATE with absolute counter values. Idempotent — calling twice
/// with the same `ScanProgress` leaves the row unchanged.
pub async fn update_progress<'e, E>(executor: E, id: i64, progress: ScanProgress) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE scan_runs
           SET files_seen = ?, files_added = ?, files_updated = ?,
               files_matched = ?, files_needs_review = ?, files_unmatched = ?
           WHERE id = ?"#,
        progress.files_seen,
        progress.files_added,
        progress.files_updated,
        progress.files_matched,
        progress.files_needs_review,
        progress.files_unmatched,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn complete<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE scan_runs
           SET status = 'completed', finished_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// One-shot UPDATE that sets the final counters, marks the run completed,
/// and stamps `finished_at`. Use this from the scanner's success path so
/// the row goes from `running` to `completed` in a single query (no
/// partial-progress window where a card would render zero counts).
pub async fn complete_with_stats<'e, E>(executor: E, id: i64, stats: ScanCompletion) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE scan_runs
           SET status = 'completed',
               finished_at = CURRENT_TIMESTAMP,
               files_seen = ?, files_added = ?, files_updated = ?,
               files_matched = ?, files_needs_review = ?, files_unmatched = ?
           WHERE id = ?"#,
        stats.files_seen,
        stats.files_added,
        stats.files_updated,
        stats.files_matched,
        stats.files_needs_review,
        stats.files_unmatched,
        id,
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Startup sweep: mark every row currently in `running` as `failed` with
/// the given error message. Returns the number of rows touched. Called
/// once at server boot — nothing should be `running` then because the
/// process that wrote it is gone.
pub async fn mark_running_as_failed<'e, E>(executor: E, error_message: &str) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE scan_runs
           SET status = 'failed',
               finished_at = CURRENT_TIMESTAMP,
               error_message = ?
           WHERE status = 'running'"#,
        error_message
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn fail<'e, E>(executor: E, id: i64, error_message: &str) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE scan_runs
           SET status = 'failed', finished_at = CURRENT_TIMESTAMP,
               error_message = ?
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
