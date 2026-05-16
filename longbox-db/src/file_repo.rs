use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

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
                  cached_comicinfo_xml, cached_at AS "cached_at: _"
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
                  cached_comicinfo_xml, cached_at AS "cached_at: _"
           FROM files WHERE library_root_id = ? AND path_relative = ?"#,
        library_root_id,
        path_relative
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn list_by_library_root<'e, E>(
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
                  cached_comicinfo_xml, cached_at AS "cached_at: _"
           FROM files WHERE library_root_id = ? ORDER BY path_relative"#,
        library_root_id
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn list_by_status<'e, E>(executor: E, status: &str) -> Result<Vec<FileRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                  path_relative, size_bytes AS "size_bytes!: i64",
                  mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                  match_method, match_confidence, status,
                  cached_comicinfo_xml, cached_at AS "cached_at: _"
           FROM files WHERE status = ? ORDER BY path_relative"#,
        status
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Hot path for `Scanner::rematch_unmatched`. Returns every unmatched file
/// in the given library root, regardless of which series it might belong to.
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
                  cached_comicinfo_xml, cached_at AS "cached_at: _"
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
                  cached_comicinfo_xml, cached_at AS "cached_at: _"
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
    let row = sqlx::query_as!(
        FileRow,
        r#"INSERT INTO files (issue_id, library_root_id, path_relative,
                              size_bytes, mtime, last_scanned_at,
                              match_method, match_confidence, status,
                              cached_comicinfo_xml, cached_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                     path_relative, size_bytes AS "size_bytes!: i64",
                     mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                     match_method, match_confidence, status,
                     cached_comicinfo_xml, cached_at AS "cached_at: _""#,
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
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update<'e, E>(executor: E, id: i64, patch: FileUpdate) -> Result<FileRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        FileRow,
        r#"UPDATE files
           SET issue_id = ?, size_bytes = ?, mtime = ?, last_scanned_at = ?,
               match_method = ?, match_confidence = ?, status = ?,
               cached_comicinfo_xml = ?, cached_at = ?
           WHERE id = ?
           RETURNING id AS "id!: i64", issue_id, library_root_id AS "library_root_id!: i64",
                     path_relative, size_bytes AS "size_bytes!: i64",
                     mtime AS "mtime: _", last_scanned_at AS "last_scanned_at: _",
                     match_method, match_confidence, status,
                     cached_comicinfo_xml, cached_at AS "cached_at: _""#,
        patch.issue_id,
        patch.size_bytes,
        patch.mtime,
        patch.last_scanned_at,
        patch.match_method,
        patch.match_confidence,
        patch.status,
        patch.cached_comicinfo_xml,
        patch.cached_at,
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
