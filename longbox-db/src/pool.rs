//! SQLite connection-pool setup. `open(url)` is the single entry point: it
//! constructs the pool with the project's standard pragmas, then runs any
//! pending migrations.

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::error::{DbError, Result};

/// Open (or create) a SQLite database, apply pragmas, run pending migrations,
/// return the pool.
///
/// Accepted URL forms:
/// - `":memory:"` — in-memory database. Pool size pinned to 1 so all callers
///   share the same database state (multiple connections to `:memory:` see
///   independent DBs).
/// - `"sqlite::memory:"` — explicit URL form of the above.
/// - `"sqlite:./path/to.db"` — file-backed, SQLx URL form.
/// - `"./path/to.db"` or `"path/to.db"` — bare path, normalized to `sqlite://…`.
pub async fn open(url: &str) -> Result<SqlitePool> {
    let is_memory = matches!(url, ":memory:" | "sqlite::memory:");

    let normalized = match url {
        ":memory:" => "sqlite::memory:".to_string(),
        s if s.starts_with("sqlite:") => s.to_string(),
        s => format!("sqlite://{s}"),
    };

    let options = SqliteConnectOptions::from_str(&normalized)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_millis(5_000));

    let pool_options = if is_memory {
        // Each fresh connection to `:memory:` gets its own DB, so the pool
        // must hold exactly one and reuse it.
        SqlitePoolOptions::new().max_connections(1)
    } else {
        SqlitePoolOptions::new()
    };

    let pool = pool_options.connect_with(options).await?;

    let migrator = sqlx::migrate!("./migrations");
    migrator
        .run(&pool)
        .await
        .map_err(|e| DbError::MigrationFailed(e.to_string()))?;

    // Bug 5 defense: assert that what the binary embedded matches what
    // the DB applied. Catches the silent-gap case (migrate.run() returns
    // Ok but the new migrations weren't actually applied) regardless of
    // mechanism — stale image, sqlx anomaly, cache quirk. Loudness-not-
    // silence: refuse to boot on divergence, name the likely fix.
    let embedded_max: i64 = migrator.iter().map(|m| m.version).max().unwrap_or(0);
    let embedded_count: i64 = migrator.iter().count() as i64;
    let applied_max: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await?;
    let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await?;
    if embedded_max != applied_max || embedded_count != applied_count {
        return Err(DbError::MigrationGap {
            embedded_max,
            embedded_count,
            applied_max,
            applied_count,
        });
    }

    Ok(pool)
}
