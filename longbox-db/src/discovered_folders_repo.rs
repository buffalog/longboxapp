//! Repository for `discovered_folders` — top-level library subfolders
//! that look series-shaped but don't resolve to any tracked series.
//!
//! The scanner upserts detections here; the reconciliation view lists
//! the non-dismissed ones for the user to add or dismiss.
//!
//! **Dismiss is split into two sources** (A.9 F6 hot-fix):
//!   - `dismissed_at`      — user-permanent. Set by [`dismiss`], called
//!     only from the /api/reconcile/dismiss route (the explicit
//!     Dismiss button in /library/tidy). Upsert preserves it; the row
//!     stays hidden until manually un-dismissed.
//!   - `auto_dismissed_at` — state-derived. Set by [`auto_dismiss`]
//!     (post-add, post-convert) and [`auto_dismiss_not_in`] (scanner
//!     F6 scan-end). Upsert clears it on re-detection so an
//!     auto-dismissed folder resurfaces the moment scanner detection
//!     re-qualifies it.
//!
//! A folder hides from the list when *either* column is set. The
//! split fixes the original F6 trap: pre-split, the upsert's
//! `WHERE dismissed_at IS NULL` guard silently swallowed any
//! re-detection of an auto-dismissed folder, stranding it forever.

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
    /// User-permanent dismiss: set when the user explicitly opts out
    /// of tracking this folder via /api/reconcile/dismiss. Preserved
    /// by [`upsert`] — once set, the row stays hidden until manually
    /// un-dismissed.
    pub dismissed_at: Option<PrimitiveDateTime>,
    /// State-derived dismiss: set by [`auto_dismiss`] (post-add,
    /// post-convert) or [`auto_dismiss_not_in`] (scanner F6).
    /// Cleared by [`upsert`] on re-detection so a folder resurfaces
    /// the moment scanner detection re-qualifies it.
    pub auto_dismissed_at: Option<PrimitiveDateTime>,
    pub file_count: i64,
}

/// Scanner input for [`upsert`].
#[derive(Debug, Clone)]
pub struct DiscoveredFolder {
    pub folder_name: String,
    pub file_count: i64,
}

/// Record a detected folder. Inserts a new row, or — on conflict —
/// refreshes `last_seen_at`, `file_count`, and clears
/// `auto_dismissed_at` so an auto-dismissed folder resurfaces. A
/// **user-permanently dismissed** folder is left untouched: the
/// `WHERE dismissed_at IS NULL` guard makes re-detection a clean
/// no-op against rows the user explicitly opted out of.
pub async fn upsert<'e, E>(executor: E, input: DiscoveredFolder) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO discovered_folders (folder_name, file_count)
           VALUES (?, ?)
           ON CONFLICT(folder_name) DO UPDATE SET
               last_seen_at = CURRENT_TIMESTAMP,
               file_count = excluded.file_count,
               auto_dismissed_at = NULL
           WHERE dismissed_at IS NULL"#,
        input.folder_name,
        input.file_count,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Non-dismissed discovered folders, oldest discovery first — the
/// reconciliation view's working set. A row is hidden when *either*
/// `dismissed_at` (user-permanent) or `auto_dismissed_at`
/// (state-derived) is set.
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
                  auto_dismissed_at AS "auto_dismissed_at: _",
                  file_count AS "file_count!: i64"
           FROM discovered_folders
           WHERE dismissed_at IS NULL AND auto_dismissed_at IS NULL
           ORDER BY first_seen_at ASC, folder_name ASC"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// **User-permanent** dismiss for a set of folders. Called only from
/// /api/reconcile/dismiss — the explicit Dismiss button in
/// /library/tidy. Writes `dismissed_at`, which [`upsert`] preserves;
/// the row stays hidden until manually un-dismissed. Idempotent —
/// already-user-dismissed rows are left as-is. Returns the count of
/// rows newly user-dismissed.
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

/// **State-derived (auto)** dismiss for a set of folders. Called from
/// reconcile's `add_one` (after a CV-add resolves the folder) and
/// `convert_one_folder` (after bulk-convert tracks the folder).
/// Writes `auto_dismissed_at`, which [`upsert`] clears on
/// re-detection — so if the folder later re-qualifies as untracked
/// (series removed, files unmatched again) it resurfaces in
/// /api/reconcile/untracked automatically. Idempotent against
/// already-auto-dismissed rows. Returns the count of rows newly
/// auto-dismissed.
pub async fn auto_dismiss<'e, E>(executor: E, folder_names: &[String]) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    if folder_names.is_empty() {
        return Ok(0);
    }
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "UPDATE discovered_folders SET auto_dismissed_at = CURRENT_TIMESTAMP \
         WHERE auto_dismissed_at IS NULL AND folder_name IN (",
    );
    let mut separated = qb.separated(", ");
    for name in folder_names {
        separated.push_bind(name);
    }
    separated.push_unseparated(")");
    let result = qb.build().execute(executor).await?;
    Ok(result.rows_affected())
}

/// Auto-dismiss every still-`auto_dismissed_at`-NULL row whose folder
/// is NOT in `keep` — the scanner F6 call at scan end. Resolves
/// stale rows whose files have since been resolved or whose folder is
/// no longer on disk. Writes `auto_dismissed_at` only; user-permanent
/// dismisses are left untouched.
///
/// An empty `keep` auto-dismisses every still-open row — correct
/// when the scan finds zero unresolved folders.
///
/// Scoped per-scan-root only by what the caller passed: with multiple
/// library roots, scanning root A would auto-dismiss folders from
/// root B too (the table has no library_root_id). We accept that
/// today — one-root deployment, consistent with
/// `refresh_last_matched_counts` and `tick_empty_scan_counters`.
pub async fn auto_dismiss_not_in<'e, E>(
    executor: E,
    keep: &std::collections::HashSet<String>,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "UPDATE discovered_folders SET auto_dismissed_at = CURRENT_TIMESTAMP \
         WHERE auto_dismissed_at IS NULL",
    );
    if !keep.is_empty() {
        qb.push(" AND folder_name NOT IN (");
        let mut separated = qb.separated(", ");
        for name in keep {
            separated.push_bind(name);
        }
        separated.push_unseparated(")");
    }
    let result = qb.build().execute(executor).await?;
    Ok(result.rows_affected())
}
