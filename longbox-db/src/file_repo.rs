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

/// One present file belonging to an issue that has more than one present
/// file — a candidate in a duplicate-file group. Carries the denormalized
/// series title + issue number (for display) and what the resolver needs:
/// `library_root_id`, to resolve the absolute path for deletion. Rows for one
/// issue arrive contiguous and `id`-ascending, matching the app's served-file
/// tiebreak.
///
/// Deliberately does NOT carry `cached_comicinfo_xml`. The web layer decides
/// what each file *is* from its filename alone, because the embedded
/// ComicInfo `<Number>` is the thing that mis-files these in the first place —
/// and it gates a permanent delete. Not fetching it is the cheapest way to
/// guarantee nobody reintroduces it as a fallback.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct DuplicateFileCandidate {
    pub issue_id: i64,
    /// The series the issue belongs to — the search space for a mismatched
    /// file's correct issue row.
    pub series_id: i64,
    pub series_title: String,
    pub issue_number: String,
    pub file_id: i64,
    pub path_relative: String,
    pub size_bytes: i64,
    /// FileStatus as TEXT; the served-file pick prefers `owned`.
    pub status: String,
    pub library_root_id: i64,
    /// Stored content digest and the file version it was computed against.
    /// The web layer must re-stat the file and confirm the stamp still
    /// matches before believing the digest — see
    /// [`HashCandidate::has_fresh_digest`] for why the catalog's own
    /// size/mtime are not sufficient evidence.
    pub content_blake3: Option<String>,
    pub hashed_size_bytes: Option<i64>,
    pub hashed_mtime: Option<PrimitiveDateTime>,
}

/// Re-point one file at a different issue, as a human's explicit manual
/// decision. Touches only the five match columns — never the disk-state ones
/// (size/mtime/is_present/last_seen_at) — so it can't lose a concurrent scan's
/// update by writing back a stale snapshot of the whole row.
///
/// Compare-and-swap on `expect_issue_id`: if a scan (or another request)
/// re-pointed the row since the caller validated it, this matches zero rows
/// and returns `false` rather than stomping the newer value. The caller turns
/// that into a refusal.
pub async fn repoint_manual<'e, E>(
    executor: E,
    file_id: i64,
    expect_issue_id: i64,
    new_issue_id: i64,
    now: PrimitiveDateTime,
) -> Result<bool>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE files
              SET issue_id = ?,
                  match_method = 'manual',
                  match_confidence = 1.0,
                  status = 'owned',
                  matched_at = ?
            WHERE id = ? AND issue_id = ?"#,
        new_issue_id,
        now,
        file_id,
        expect_issue_id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Every file sharing one content digest, in id order.
///
/// Byte-identical copies, which is what makes a content-duplicate
/// group. Deliberately unfiltered by `status` or `is_present`: a group
/// can contain an `ignored` copy with no `issue_id` at all, and that
/// copy is exactly the kind a caller may want to remove.
pub async fn list_by_content_hash<'e, E>(executor: E, digest: &str) -> Result<Vec<FileRow>>
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
           FROM files WHERE content_blake3 = ? ORDER BY id"#,
        digest
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Count of issues that have more than one `is_present = 1` file — the total
/// number of duplicate-file groups, for the paginated detector's `total`.
pub async fn count_duplicate_file_groups<'e, E>(executor: E) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM (
               SELECT issue_id FROM files
               WHERE is_present = 1 AND issue_id IS NOT NULL
               GROUP BY issue_id HAVING COUNT(*) > 1
           )"#
    )
    .fetch_one(executor)
    .await?;
    Ok(row.n)
}

/// Every present-file candidate for one page of duplicate-file groups.
///
/// The inner subquery selects the page's `issue_id`s (those with >1 present
/// file, ordered by `issue_id` for stable pagination, `LIMIT`/`OFFSET`); the
/// outer query returns all present files for those issues, ordered
/// `issue_id, id` so each group's rows are contiguous and `id`-ascending.
/// Classification (are these truly the same issue?) and the keep-suggestion
/// are the web layer's job — this repo only surfaces the raw candidates.
pub async fn duplicate_file_candidates_page<'e, E>(
    executor: E,
    limit: i64,
    offset: i64,
) -> Result<Vec<DuplicateFileCandidate>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        DuplicateFileCandidate,
        r#"SELECT f.issue_id AS "issue_id!: i64",
                  i.series_id AS "series_id!: i64",
                  s.title AS series_title,
                  i.number AS issue_number,
                  f.id AS "file_id!: i64",
                  f.path_relative,
                  f.size_bytes AS "size_bytes!: i64",
                  f.status,
                  f.library_root_id AS "library_root_id!: i64",
                  f.content_blake3,
                  f.hashed_size_bytes,
                  f.hashed_mtime AS "hashed_mtime?: PrimitiveDateTime"
           FROM files f
           JOIN issues i ON f.issue_id = i.id
           JOIN series s ON i.series_id = s.id
           WHERE f.is_present = 1
             AND f.issue_id IN (
                 SELECT ff.issue_id FROM files ff
                 WHERE ff.is_present = 1 AND ff.issue_id IS NOT NULL
                 GROUP BY ff.issue_id HAVING COUNT(*) > 1
                 ORDER BY ff.issue_id
                 LIMIT ? OFFSET ?
             )
           ORDER BY f.issue_id, f.id"#,
        limit,
        offset
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

// -------- content-identity hashing --------

/// A present file that shares its byte size with at least one other present
/// file. Two files of differing size cannot be byte-identical, so this is the
/// complete candidate set for content-duplicate detection — and on a healthy
/// library it is a tiny fraction of the whole (80 of 7102 on the library this
/// was built against).
///
/// Carries the stored digest and its version stamp so the caller can decide
/// freshness without a second query — see [`HashCandidate::has_fresh_digest`].
#[derive(Debug, Clone, PartialEq)]
pub struct HashCandidate {
    pub file_id: i64,
    pub library_root_id: i64,
    pub path_relative: String,
    /// Size per the catalog. May lag the file on disk if a scan hasn't run.
    pub size_bytes: i64,
    /// mtime per the catalog. Same caveat.
    pub mtime: PrimitiveDateTime,
    pub content_blake3: Option<String>,
    pub hashed_size_bytes: Option<i64>,
    pub hashed_mtime: Option<PrimitiveDateTime>,
}

impl HashCandidate {
    /// True when the stored digest can be trusted for the bytes currently on
    /// disk: a digest exists AND the size/mtime it was computed against equal
    /// what the caller just observed **by stat-ing the file**.
    ///
    /// The observed values are parameters, not the row's own `size_bytes` /
    /// `mtime`, and that distinction is the whole point. Comparing the stamp
    /// against sibling catalog columns would only prove "the catalog hasn't
    /// changed since we hashed" — and the catalog is refreshed by a scan that
    /// runs once a day. Any edit made between two scans leaves the catalog
    /// agreeing with the stamp while the bytes underneath have changed, which
    /// would report two DIFFERENT files as identical and put a delete button
    /// next to that claim. Disk is the authority; the catalog is a cache.
    ///
    /// Deliberately conservative — any missing piece reads as stale and costs
    /// only a re-hash.
    pub fn has_fresh_digest(&self, observed_size: i64, observed_mtime: PrimitiveDateTime) -> bool {
        match (
            &self.content_blake3,
            self.hashed_size_bytes,
            self.hashed_mtime,
        ) {
            (Some(digest), Some(size), Some(mtime)) => {
                !digest.is_empty() && size == observed_size && mtime == observed_mtime
            }
            _ => false,
        }
    }
}

/// Unlink every file pointing at any issue of `series_id`, leaving each at
/// `issue_id = NULL, status = 'needs_review'`. Returns the row count.
///
/// MUST be called BEFORE the series' issues are deleted. `files.issue_id` is
/// the only record of the association and `ON DELETE SET NULL` destroys it;
/// afterwards the affected rows cannot be identified at all, which is why the
/// caller that ran this after the delete had to fall back to matching a
/// folder-name prefix — and missed every file living anywhere else.
///
/// `needs_review` is the correct destination: it is what `rematch_for_series`
/// selects, so the rows heal on the next pass. Leaving them `owned` with no
/// issue produces a state `classify_status` cannot generate and no consumer
/// can reach.
pub async fn unlink_by_series<'e, E>(executor: E, series_id: i64) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE files
             SET issue_id = NULL,
                 status = 'needs_review'
           WHERE issue_id IN (SELECT id FROM issues WHERE series_id = ?)"#,
        series_id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// A file's stored digest and the file version it was computed against.
///
/// Fetched separately from [`FileRow`] rather than widening it: only the
/// delete path needs this, and adding three columns to `FileRow` would change
/// eight unrelated queries for one caller's benefit.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentStamp {
    pub file_id: i64,
    pub content_blake3: Option<String>,
    pub hashed_size_bytes: Option<i64>,
    pub hashed_mtime: Option<PrimitiveDateTime>,
}

/// Digest stamps for every file sharing one content digest.
///
/// Deliberately not filtered on `is_present`: the caller is deciding
/// whether a copy really exists, and must stat rather than trust that
/// column. Returning only "present" rows would pre-answer the question
/// with the catalog value the caller is trying to avoid.
pub async fn content_stamps_by_content_hash<'e, E>(
    executor: E,
    digest: &str,
) -> Result<Vec<ContentStamp>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        ContentStamp,
        r#"SELECT id AS "file_id!: i64",
                  content_blake3,
                  hashed_size_bytes,
                  hashed_mtime AS "hashed_mtime?: PrimitiveDateTime"
           FROM files
           WHERE content_blake3 = ?"#,
        digest
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Digest stamps for every present file on one issue.
pub async fn content_stamps_by_issue<'e, E>(executor: E, issue_id: i64) -> Result<Vec<ContentStamp>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        ContentStamp,
        r#"SELECT id AS "file_id!: i64",
                  content_blake3,
                  hashed_size_bytes,
                  hashed_mtime AS "hashed_mtime?: PrimitiveDateTime"
           FROM files
           WHERE issue_id = ? AND is_present = 1"#,
        issue_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Every present file whose size is shared with another present file.
///
/// The whole candidate set, not a page: it is bounded by real duplication
/// rather than library size, and the caller needs all of it at once to group
/// by digest. Ordered `size_bytes, id` so same-size rows are contiguous.
pub async fn size_collision_candidates<'e, E>(executor: E) -> Result<Vec<HashCandidate>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        HashCandidate,
        r#"SELECT f.id AS "file_id!: i64",
                  f.library_root_id AS "library_root_id!: i64",
                  f.path_relative,
                  f.size_bytes AS "size_bytes!: i64",
                  f.mtime AS "mtime!: PrimitiveDateTime",
                  f.content_blake3,
                  f.hashed_size_bytes,
                  f.hashed_mtime AS "hashed_mtime?: PrimitiveDateTime"
           FROM files f
           WHERE f.is_present = 1
             AND f.size_bytes IN (
                 SELECT ff.size_bytes FROM files ff
                 WHERE ff.is_present = 1
                 GROUP BY ff.size_bytes HAVING COUNT(*) > 1
             )
           ORDER BY f.size_bytes, f.id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Store the identity label read from inside the archive, alongside the
/// digest pass that opened it. `kind` is `"dir"` or `"page"`.
///
/// Written in the same pass as the digest and validated by the same stamp:
/// both are derived from the same bytes, so a file that changed since hashing
/// has an untrustworthy label for the same reason it has an untrustworthy
/// digest. `None` clears a previously-stored label when the archive no longer
/// yields one, so a stale label can never outlive the evidence for it.
pub async fn set_archive_label<'e, E>(
    executor: E,
    file_id: i64,
    label: Option<&str>,
    kind: Option<&str>,
) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"UPDATE files SET archive_label = ?, archive_label_kind = ? WHERE id = ?"#,
        label,
        kind,
        file_id
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Store a digest together with the file version it was computed against.
///
/// `size` and `mtime` must be what was observed on disk at hash time, NOT the
/// catalog's values. If the two disagree the catalog is stale, and stamping
/// the disk truth makes the row read as stale on the next pass (so it is
/// re-hashed) instead of falsely claiming a digest for bytes it never saw.
pub async fn set_content_hash<'e, E>(
    executor: E,
    file_id: i64,
    digest: &str,
    size: i64,
    mtime: PrimitiveDateTime,
) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"UPDATE files
              SET content_blake3 = ?, hashed_size_bytes = ?, hashed_mtime = ?
            WHERE id = ?"#,
        digest,
        size,
        mtime,
        file_id
    )
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn candidate(
        digest: Option<&str>,
        hashed_size: Option<i64>,
        hashed_mtime: Option<PrimitiveDateTime>,
    ) -> HashCandidate {
        HashCandidate {
            file_id: 1,
            library_root_id: 1,
            path_relative: "S/S 001.cbz".into(),
            size_bytes: 100,
            mtime: datetime!(2026-07-01 10:00:00),
            content_blake3: digest.map(str::to_owned),
            hashed_size_bytes: hashed_size,
            hashed_mtime,
        }
    }

    const DISK_SIZE: i64 = 100;
    const DISK_MTIME: PrimitiveDateTime = datetime!(2026-07-01 10:00:00);

    #[test]
    fn digest_is_fresh_when_stamp_matches_what_is_on_disk() {
        let c = candidate(Some("abc"), Some(100), Some(datetime!(2026-07-01 10:00:00)));
        assert!(c.has_fresh_digest(DISK_SIZE, DISK_MTIME));
    }

    #[test]
    fn digest_is_stale_when_disk_size_or_mtime_moved() {
        let c = candidate(Some("abc"), Some(100), Some(datetime!(2026-07-01 10:00:00)));
        // Disk grew — same mtime is not enough.
        assert!(!c.has_fresh_digest(101, DISK_MTIME));
        // Disk mtime moved — same size is not enough.
        assert!(!c.has_fresh_digest(DISK_SIZE, datetime!(2026-07-02 10:00:00)));
    }

    /// The regression that matters: a file edited between two nightly scans
    /// leaves the CATALOG agreeing with the stamp while the bytes on disk have
    /// changed. Freshness must follow disk, never the catalog, or the
    /// integrity UI offers to delete a file that is no longer a duplicate.
    #[test]
    fn stale_catalog_does_not_make_a_stale_digest_look_fresh() {
        // Row and stamp agree at 100 bytes — the catalog is simply out of date.
        let c = candidate(Some("abc"), Some(100), Some(datetime!(2026-07-01 10:00:00)));
        assert_eq!(c.size_bytes, 100, "catalog still says 100");
        // Disk says otherwise. The digest describes bytes that are gone.
        assert!(
            !c.has_fresh_digest(60_000_000, datetime!(2026-07-22 10:00:00)),
            "digest must be stale when disk disagrees, even though the catalog matches"
        );
    }

    #[test]
    fn digest_is_stale_when_any_piece_is_missing() {
        assert!(
            !candidate(None, Some(100), Some(DISK_MTIME)).has_fresh_digest(DISK_SIZE, DISK_MTIME)
        );
        assert!(
            !candidate(Some("abc"), None, Some(DISK_MTIME)).has_fresh_digest(DISK_SIZE, DISK_MTIME)
        );
        assert!(!candidate(Some("abc"), Some(100), None).has_fresh_digest(DISK_SIZE, DISK_MTIME));
        // An empty digest is a write that went wrong, not a valid hash.
        assert!(!candidate(Some(""), Some(100), Some(DISK_MTIME))
            .has_fresh_digest(DISK_SIZE, DISK_MTIME));
    }

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
