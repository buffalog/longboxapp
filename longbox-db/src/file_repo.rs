use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct FileRow {
    pub id: i64,
    pub issue_id: Option<i64>,
    pub library_root_id: i64,
    pub path_relative: String,
    pub size_bytes: i64,
    pub mtime: PrimitiveDateTime,
    pub last_scanned_at: PrimitiveDateTime,
    /// Enum stored as TEXT. Convert with [`longbox_core::MatchMethod::from_db_str`].
    pub match_method: String,
    pub match_confidence: f64,
    /// Enum stored as TEXT. Convert with [`longbox_core::FileStatus::from_db_str`].
    pub status: String,
    pub cached_comicinfo_xml: Option<String>,
    pub cached_at: Option<PrimitiveDateTime>,
    /// `true` iff the most recent scan visited this row on disk. Flipped to
    /// `false` by [`mark_files_not_seen_since`].
    pub is_present: bool,
    /// Timestamp of the most recent scan that visited this row.
    pub last_seen_at: PrimitiveDateTime,
    /// Wall-clock time when `issue_id` was most recently set to its current
    /// value. NULL when `issue_id` is NULL, and on pre-Task-3 rows that
    /// were matched before the column existed (no backfill).
    pub matched_at: Option<PrimitiveDateTime>,
}

#[derive(Debug, Clone)]
pub struct NewFile {
    pub issue_id: Option<i64>,
    pub library_root_id: i64,
    pub path_relative: String,
    pub size_bytes: i64,
    pub mtime: PrimitiveDateTime,
    pub last_scanned_at: PrimitiveDateTime,
    pub match_method: String,
    pub match_confidence: f64,
    pub status: String,
    pub cached_comicinfo_xml: Option<String>,
    pub cached_at: Option<PrimitiveDateTime>,
    pub is_present: bool,
    pub last_seen_at: PrimitiveDateTime,
    /// Caller-supplied; should be `Some(now)` when `issue_id.is_some()`,
    /// `None` otherwise. Use [`next_matched_at`] when in doubt.
    pub matched_at: Option<PrimitiveDateTime>,
}

/// All fields except identity (`id`, `library_root_id`, `path_relative`) are
/// settable. `issue_id` accepts `None` to clear (used by the "ignore" flow).
#[derive(Debug, Clone)]
pub struct FileUpdate {
    pub issue_id: Option<i64>,
    pub size_bytes: i64,
    pub mtime: PrimitiveDateTime,
    pub last_scanned_at: PrimitiveDateTime,
    pub match_method: String,
    pub match_confidence: f64,
    pub status: String,
    pub cached_comicinfo_xml: Option<String>,
    pub cached_at: Option<PrimitiveDateTime>,
    pub is_present: bool,
    pub last_seen_at: PrimitiveDateTime,
    /// See [`next_matched_at`]. Repo persists the value verbatim — the
    /// caller is responsible for computing it correctly relative to the
    /// existing row's issue_id and matched_at.
    pub matched_at: Option<PrimitiveDateTime>,
}

/// Computes the new `matched_at` for a file write, per Task 3 rule:
///
/// - `None -> Some(_)`            : the file just became matched -> `Some(now)`
/// - `Some(a) -> Some(b)`, a != b : remapped to a different issue -> `Some(now)`
/// - `Some(a) -> Some(a)`         : same match, just re-confirmed -> `old`
/// - `_ -> None`                  : cleared (mark-ignored / revert)  -> `None`
///
/// `old_issue_id` and `old_matched_at` come from the existing row; the
/// caller has them in scope at every site that mutates a file row.
pub fn next_matched_at(
    old_issue_id: Option<i64>,
    new_issue_id: Option<i64>,
    old_matched_at: Option<PrimitiveDateTime>,
    now: PrimitiveDateTime,
) -> Option<PrimitiveDateTime> {
    match (old_issue_id, new_issue_id) {
        (_, None) => None,
        (None, Some(_)) => Some(now),
        (Some(a), Some(b)) if a != b => Some(now),
        (Some(_), Some(_)) => old_matched_at,
    }
}

pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<FileRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                  path_relative, size_bytes AS "size_bytes!: i64",
                  mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                  match_method, match_confidence, status,
                  cached_comicinfo_xml, cached_at AS "cached_at: _",
                  is_present AS "is_present!: bool",
                  last_seen_at AS "last_seen_at: _",
                  matched_at AS "matched_at: _"
           FROM files WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Hot path: scanner reads this for every visited file to decide insert vs
/// update vs skip-via-cache-hit.
pub async fn find_by_path<'e, E>(
    executor: E,
    library_root_id: i64,
    path_relative: &str,
) -> Result<Option<FileRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                  path_relative, size_bytes AS "size_bytes!: i64",
                  mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                  match_method, match_confidence, status,
                  cached_comicinfo_xml, cached_at AS "cached_at: _",
                  is_present AS "is_present!: bool",
                  last_seen_at AS "last_seen_at: _",
                  matched_at AS "matched_at: _"
           FROM files WHERE library_root_id = ? AND path_relative = ?"#,
        library_root_id,
        path_relative
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn list_by_library_root<'e, E>(executor: E, library_root_id: i64) -> Result<Vec<FileRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                  path_relative, size_bytes AS "size_bytes!: i64",
                  mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                  match_method, match_confidence, status,
                  cached_comicinfo_xml, cached_at AS "cached_at: _",
                  is_present AS "is_present!: bool",
                  last_seen_at AS "last_seen_at: _",
                  matched_at AS "matched_at: _"
           FROM files WHERE library_root_id = ? ORDER BY path_relative"#,
        library_root_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// List files in `library_root_id` whose `status` matches the given enum
/// string (use [`longbox_core::FileStatus::as_db_str`] to construct).
pub async fn list_by_status<'e, E>(
    executor: E,
    library_root_id: i64,
    status: &str,
) -> Result<Vec<FileRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                  path_relative, size_bytes AS "size_bytes!: i64",
                  mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                  match_method, match_confidence, status,
                  cached_comicinfo_xml, cached_at AS "cached_at: _",
                  is_present AS "is_present!: bool",
                  last_seen_at AS "last_seen_at: _",
                  matched_at AS "matched_at: _"
           FROM files WHERE library_root_id = ? AND status = ?
           ORDER BY path_relative"#,
        library_root_id,
        status
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Hot path for `Scanner::rematch_unmatched`. Returns every unmatched file
/// in the given library root.
pub async fn list_unmatched_for_series<'e, E>(
    executor: E,
    library_root_id: i64,
) -> Result<Vec<FileRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                  path_relative, size_bytes AS "size_bytes!: i64",
                  mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                  match_method, match_confidence, status,
                  cached_comicinfo_xml, cached_at AS "cached_at: _",
                  is_present AS "is_present!: bool",
                  last_seen_at AS "last_seen_at: _",
                  matched_at AS "matched_at: _"
           FROM files
           WHERE library_root_id = ? AND status = 'unmatched'
           ORDER BY path_relative"#,
        library_root_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn list_by_issue<'e, E>(executor: E, issue_id: i64) -> Result<Vec<FileRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                  path_relative, size_bytes AS "size_bytes!: i64",
                  mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                  match_method, match_confidence, status,
                  cached_comicinfo_xml, cached_at AS "cached_at: _",
                  is_present AS "is_present!: bool",
                  last_seen_at AS "last_seen_at: _",
                  matched_at AS "matched_at: _"
           FROM files WHERE issue_id = ? ORDER BY path_relative"#,
        issue_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn insert<'e, E>(executor: E, input: NewFile) -> Result<FileRow>
where
    E: SqliteExecutor<'e>,
{
    let is_present = i64::from(input.is_present);
    let row = sqlx::query_as!(
        FileRow,
        r#"INSERT INTO files (issue_id, library_root_id, path_relative,
                              size_bytes, mtime, last_scanned_at,
                              match_method, match_confidence, status,
                              cached_comicinfo_xml, cached_at,
                              is_present, last_seen_at, matched_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                     path_relative, size_bytes AS "size_bytes!: i64",
                     mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                     match_method, match_confidence, status,
                     cached_comicinfo_xml, cached_at AS "cached_at: _",
                     is_present AS "is_present!: bool",
                     last_seen_at AS "last_seen_at: _",
                     matched_at AS "matched_at: _""#,
        input.issue_id,
        input.library_root_id,
        input.path_relative,
        input.size_bytes,
        input.mtime,
        input.last_scanned_at,
        input.match_method,
        input.match_confidence,
        input.status,
        input.cached_comicinfo_xml,
        input.cached_at,
        is_present,
        input.last_seen_at,
        input.matched_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(executor: E, id: i64, patch: FileUpdate) -> Result<FileRow>
where
    E: SqliteExecutor<'e>,
{
    let is_present = i64::from(patch.is_present);
    let row = sqlx::query_as!(
        FileRow,
        r#"UPDATE files
           SET issue_id = ?, size_bytes = ?, mtime = ?, last_scanned_at = ?,
               match_method = ?, match_confidence = ?, status = ?,
               cached_comicinfo_xml = ?, cached_at = ?,
               is_present = ?, last_seen_at = ?, matched_at = ?
           WHERE id = ?
           RETURNING id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                     path_relative, size_bytes AS "size_bytes!: i64",
                     mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                     match_method, match_confidence, status,
                     cached_comicinfo_xml, cached_at AS "cached_at: _",
                     is_present AS "is_present!: bool",
                     last_seen_at AS "last_seen_at: _",
                     matched_at AS "matched_at: _""#,
        patch.issue_id,
        patch.size_bytes,
        patch.mtime,
        patch.last_scanned_at,
        patch.match_method,
        patch.match_confidence,
        patch.status,
        patch.cached_comicinfo_xml,
        patch.cached_at,
        is_present,
        patch.last_seen_at,
        patch.matched_at,
        id
    )
    .fetch_optional(executor)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(row)
}

pub async fn delete<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM files WHERE id = ?"#, id)
        .execute(executor)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Phase B's "I just imported a file and I know exactly which issue it is"
/// path. Single repo method, idempotent on `(library_root_id,
/// path_relative)`; intended to be called from `longbox-postprocess`
/// after the .cbz has been written to its final library location.
///
/// Always sets:
/// - `status = 'owned'`
/// - `match_method` to the caller-supplied value — `'phase_b'` for an
///   ordinary Phase B catch, `'pull_list'` for a file the pull engine
///   auto-downloaded. Either overrides a prior lower-confidence match
///   like `'filename_regex'` or `'comicinfo_xml'`; the auto-import
///   paths are the higher-confidence ones.
/// - `match_confidence = 1.0`
/// - `is_present = true`
/// - `last_scanned_at = now` / `last_seen_at = now`
///
/// `matched_at` follows the policy enforced by [`next_matched_at`]:
/// fresh on first match or re-mapped issue, preserved when the same
/// `issue_id` is re-confirmed.
///
/// `cached_comicinfo_xml` / `cached_at` are cleared because Phase B's
/// own writer just produced a fresh ComicInfo embedded in the file;
/// the scanner can re-cache from disk on the next scan if needed.
// One parameter per written column — a row-struct would only move the
// argument list, not shorten it.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_imported<'e, E>(
    executor: E,
    library_root_id: i64,
    path_relative: &str,
    series_id: i64, // currently informational; FK is via issue_id
    issue_id: i64,
    match_method: &str,
    size: i64,
    mtime: OffsetDateTime,
) -> Result<FileRow>
where
    E: SqliteExecutor<'e> + Copy,
{
    // series_id is accepted for clarity at the call site (Phase B's
    // pipeline holds both ids) and to keep the brief's signature
    // honest, but the `files` table only stores `issue_id` — the join
    // path to series is via the issues table.
    let _ = series_id;

    let mtime_p = PrimitiveDateTime::new(mtime.date(), mtime.time());
    let now_p = {
        let now = OffsetDateTime::now_utc();
        PrimitiveDateTime::new(now.date(), now.time())
    };

    let existing = find_by_path(executor, library_root_id, path_relative).await?;
    let matched_at = next_matched_at(
        existing.as_ref().and_then(|r| r.issue_id),
        Some(issue_id),
        existing.as_ref().and_then(|r| r.matched_at),
        now_p,
    );

    if let Some(row) = existing {
        let patch = FileUpdate {
            issue_id: Some(issue_id),
            size_bytes: size,
            mtime: mtime_p,
            last_scanned_at: now_p,
            match_method: match_method.to_owned(),
            match_confidence: 1.0,
            status: longbox_core::FileStatus::Owned.as_db_str().to_owned(),
            // Phase B wrote a fresh ComicInfo into the file; drop any
            // stale cache so the next scan re-reads from disk.
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now_p,
            matched_at,
        };
        update(executor, row.id, patch).await
    } else {
        let new = NewFile {
            issue_id: Some(issue_id),
            library_root_id,
            path_relative: path_relative.to_owned(),
            size_bytes: size,
            mtime: mtime_p,
            last_scanned_at: now_p,
            match_method: match_method.to_owned(),
            match_confidence: 1.0,
            status: longbox_core::FileStatus::Owned.as_db_str().to_owned(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now_p,
            matched_at,
        };
        insert(executor, new).await
    }
}

/// Delete every row for `(library_root_id, issue_id)` whose `path_relative`
/// is NOT `kept_path` and whose `is_present = 0`. Returns the purged count.
///
/// Called by Phase B immediately after [`upsert_imported`] writes the row
/// for the canonical post-move location. Catches the case where the same
/// file was previously cataloged at a different path inside the same
/// library root (e.g. `_unsorted/<basename>`) and a subsequent scan
/// flipped that row to `is_present = 0`. Without this purge the stale row
/// reappears as a phantom `needs_review` entry on the next match cycle.
///
/// The `is_present = 0` guard is load-bearing: it prevents collateral
/// damage to a legitimate second copy of the same issue that is still on
/// disk (different format, different edition, etc.). Only rows the
/// scanner has already confirmed are physically gone get purged.
pub async fn purge_absent_ghosts_for_issue<'e, E>(
    executor: E,
    library_root_id: i64,
    issue_id: i64,
    kept_path: &str,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"DELETE FROM files
           WHERE library_root_id = ?
             AND issue_id = ?
             AND path_relative != ?
             AND is_present = 0"#,
        library_root_id,
        issue_id,
        kept_path
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Mark `is_present = false` for every file in `library_root_id` whose
/// `last_seen_at` is strictly less than `cutoff`. Returns the row count.
/// Used by `Scanner::scan_full` after a full pass to flip missing files.
pub async fn mark_files_not_seen_since<'e, E>(
    executor: E,
    library_root_id: i64,
    cutoff: PrimitiveDateTime,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE files
           SET is_present = 0
           WHERE library_root_id = ?
             AND last_seen_at < ?
             AND is_present = 1"#,
        library_root_id,
        cutoff
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn matched_at_first_match() {
        let now = datetime!(2026-05-18 10:00:00);
        // None -> Some(1) → fresh match, set to now.
        assert_eq!(next_matched_at(None, Some(1), None, now), Some(now));
    }

    #[test]
    fn matched_at_changed_match() {
        let earlier = datetime!(2026-05-18 09:00:00);
        let now = datetime!(2026-05-18 10:00:00);
        // Some(1) -> Some(2) → user remapped, set to now.
        assert_eq!(
            next_matched_at(Some(1), Some(2), Some(earlier), now),
            Some(now)
        );
    }

    #[test]
    fn matched_at_unchanged_match() {
        let earlier = datetime!(2026-05-18 09:00:00);
        let now = datetime!(2026-05-18 10:00:00);
        // Some(1) -> Some(1) → same match, keep the old timestamp.
        assert_eq!(
            next_matched_at(Some(1), Some(1), Some(earlier), now),
            Some(earlier)
        );
    }

    #[test]
    fn matched_at_cleared() {
        let earlier = datetime!(2026-05-18 09:00:00);
        let now = datetime!(2026-05-18 10:00:00);
        // Any -> None → clear.
        assert_eq!(next_matched_at(Some(1), None, Some(earlier), now), None);
        assert_eq!(next_matched_at(None, None, None, now), None);
    }
}
