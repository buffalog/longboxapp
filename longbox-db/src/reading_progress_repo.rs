//! Reader position storage for the built-in comic reader. One row per issue
//! that has been opened; a missing row means "never opened", which
//! [`get_last_page`] reports as page 1. `last_page` is the reader's primary
//! position (the left page of the current spread in two-page mode).

use sqlx::SqliteExecutor;

use crate::error::Result;

/// The stored reader position for an issue, or `1` when no row exists (never
/// opened). Never signals absence as an error — the reader always has a page
/// to open to.
pub async fn get_last_page<'e, E>(executor: E, issue_id: i64) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT last_page AS "last_page!: i64" FROM reading_progress WHERE issue_id = ?"#,
        issue_id
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| r.last_page).unwrap_or(1))
}

/// Upsert the reader position for an issue, refreshing `updated_at`. Uses an
/// `ON CONFLICT` upsert rather than `INSERT OR REPLACE` so the existing row is
/// updated in place (no delete/reinsert, no cascade side effects).
pub async fn set_last_page<'e, E>(executor: E, issue_id: i64, last_page: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO reading_progress (issue_id, last_page, updated_at)
           VALUES (?, ?, CURRENT_TIMESTAMP)
           ON CONFLICT(issue_id) DO UPDATE SET
               last_page = excluded.last_page,
               updated_at = CURRENT_TIMESTAMP"#,
        issue_id,
        last_page
    )
    .execute(executor)
    .await?;
    Ok(())
}
