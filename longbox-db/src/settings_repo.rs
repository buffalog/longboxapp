use std::str::FromStr;

use sqlx::SqliteExecutor;

use crate::error::Result;

/// `settings.key` value: user-tunable owned/needs_review boundary.
pub const KEY_MATCH_CONFIDENCE_THRESHOLD: &str = "match_confidence_threshold";

/// `settings.key` value: filesystem path of the library root.
pub const KEY_LIBRARY_ROOT_PATH: &str = "library_root_path";

pub async fn get<'e, E>(executor: E, key: &str) -> Result<Option<String>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(r#"SELECT value FROM settings WHERE key = ?"#, key)
        .fetch_optional(executor)
        .await?;
    Ok(row.map(|r| r.value))
}

/// Upsert: set the value for `key`, replacing any prior value. `updated_at`
/// is bumped to the current timestamp.
pub async fn set<'e, E>(executor: E, key: &str, value: &str) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO settings (key, value) VALUES (?, ?)
           ON CONFLICT(key) DO UPDATE
           SET value = excluded.value, updated_at = CURRENT_TIMESTAMP"#,
        key,
        value
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Read `key` and parse via `FromStr`. Returns `default` when the row is
/// missing or its value fails to parse — the matcher's threshold setting
/// should never panic the scanner because someone wrote `'banana'` into the
/// table.
pub async fn get_or_default<'e, E, T>(executor: E, key: &str, default: T) -> Result<T>
where
    E: SqliteExecutor<'e>,
    T: FromStr,
{
    match get(executor, key).await? {
        Some(raw) => Ok(raw.parse::<T>().unwrap_or(default)),
        None => Ok(default),
    }
}
