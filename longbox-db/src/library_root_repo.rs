use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;
use time::PrimitiveDateTime;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRootRow {
    pub id: i64,
    pub path: String,
    pub created_at: PrimitiveDateTime,
}

#[derive(Debug, Clone)]
pub struct NewLibraryRoot {
    pub path: String,
}

pub async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<LibraryRootRow>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        LibraryRootRow,
        r#"SELECT id AS "id!: i64", path, created_at AS "created_at: _"
           FROM library_roots WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

pub async fn list_all<'e, E>(executor: E) -> Result<Vec<LibraryRootRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        LibraryRootRow,
        r#"SELECT id AS "id!: i64", path, created_at AS "created_at: _"
           FROM library_roots ORDER BY id"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

pub async fn insert<'e, E>(executor: E, input: NewLibraryRoot) -> Result<LibraryRootRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        LibraryRootRow,
        r#"INSERT INTO library_roots (path) VALUES (?)
           RETURNING id AS "id!: i64", path, created_at AS "created_at: _""#,
        input.path
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

pub async fn update_path<'e, E>(executor: E, id: i64, new_path: &str) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE library_roots SET path = ? WHERE id = ?"#,
        new_path,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}
