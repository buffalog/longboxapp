//! Per-user OPDS accounts (`opds_users` table). Admin-managed only — no
//! self-registration. The web layer's OPDS auth middleware authenticates
//! against this table; the `/api/opds/users` admin endpoints drive the CRUD.
//!
//! `created_at` / `last_seen_at` are SQLite `TEXT` (`datetime('now')`,
//! `"YYYY-MM-DD HH:MM:SS"` UTC) carried through as `String` — the UI only
//! displays them.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;

use crate::error::{DbError, Result};

/// An OPDS account as the admin UI sees it. The password hash is
/// deliberately absent — it never leaves the server. See
/// [`find_enabled_for_auth`] for the auth-path projection that carries it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct OpdsUserRow {
    pub id: i64,
    pub username: String,
    pub enabled: bool,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

/// Auth-path projection: just what the middleware needs to verify a login.
#[derive(Debug, Clone)]
pub struct OpdsUserAuth {
    pub id: i64,
    pub password_hash: String,
}

/// Every account, ordered by username (case-insensitive). Small table
/// (≤ ~10 rows in practice) — no pagination.
pub async fn list<'e, E>(executor: E) -> Result<Vec<OpdsUserRow>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        OpdsUserRow,
        r#"SELECT id AS "id!: i64", username,
                  enabled AS "enabled!: bool",
                  created_at AS "created_at!: String",
                  last_seen_at
           FROM opds_users
           ORDER BY username COLLATE NOCASE"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Create an enabled account. `password_hash` must already be bcrypt-hashed
/// (hashing lives in `longbox-opds`, the crate that owns bcrypt). A duplicate
/// username surfaces as [`DbError::UniqueViolation`] (`field: "opds_username"`)
/// — the case-insensitive `UNIQUE` index is the enforcement point.
pub async fn create<'e, E>(executor: E, username: &str, password_hash: &str) -> Result<OpdsUserRow>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        OpdsUserRow,
        r#"INSERT INTO opds_users (username, password_hash)
           VALUES (?, ?)
           RETURNING id AS "id!: i64", username,
                     enabled AS "enabled!: bool",
                     created_at AS "created_at!: String",
                     last_seen_at"#,
        username,
        password_hash
    )
    .fetch_one(executor)
    .await?;
    Ok(row)
}

/// Flip an account's `enabled` flag. `NotFound` for an unknown id.
pub async fn set_enabled<'e, E>(executor: E, id: i64, enabled: bool) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE opds_users SET enabled = ? WHERE id = ?"#,
        enabled,
        id
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Hard-delete an account. `NotFound` for an unknown id.
pub async fn delete<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(r#"DELETE FROM opds_users WHERE id = ?"#, id)
        .execute(executor)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Look up an *enabled* account by username (case-insensitive) for the auth
/// path, returning its id + bcrypt hash. Disabled accounts and unknown
/// usernames both return `None` — the middleware can't distinguish them, and
/// shouldn't.
pub async fn find_enabled_for_auth<'e, E>(
    executor: E,
    username: &str,
) -> Result<Option<OpdsUserAuth>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query_as!(
        OpdsUserAuth,
        r#"SELECT id AS "id!: i64", password_hash
           FROM opds_users
           WHERE enabled = 1 AND username = ? COLLATE NOCASE"#,
        username
    )
    .fetch_optional(executor)
    .await?;
    Ok(row)
}

/// Stamp `last_seen_at` to now on a successful auth. Best-effort from the
/// caller's view — a missing id (deleted mid-request) is a silent no-op
/// rather than an error.
pub async fn touch_last_seen<'e, E>(executor: E, id: i64) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"UPDATE opds_users SET last_seen_at = datetime('now') WHERE id = ?"#,
        id
    )
    .execute(executor)
    .await?;
    Ok(())
}
