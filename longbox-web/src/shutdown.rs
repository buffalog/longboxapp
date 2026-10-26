//! Graceful shutdown on `docker stop`.
//!
//! The binary is the container's PID 1, and the kernel drops SIGTERM for
//! PID 1 unless it installs a handler. Without one, every `docker stop`
//! waited out its 10s grace and ended in SIGKILL (exit 137), so nothing
//! closed the DB or checkpointed the WAL. `main` waits on [`signal`],
//! drains the listeners, then runs [`close_db`].

use std::time::Duration;

use longbox_db::Pool;
use sqlx::Connection;
use tracing::{info, warn};

/// Resolves on SIGTERM (`docker stop`) or Ctrl-C. A handler that can't be
/// installed never resolves, rather than shutting the server down at boot.
pub async fn signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!(target: "longbox_web", error = %e, "ctrl-c handler unavailable");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                warn!(target: "longbox_web", error = %e, "SIGTERM handler unavailable");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => info!(target: "longbox_web", "ctrl-c received; shutting down"),
        () = term => info!(target: "longbox_web", "SIGTERM received; shutting down"),
    }
}

/// Stop every background worker's DB access, then checkpoint the WAL.
///
/// One connection is detached first, so it outlives the pool. Closing the
/// pool makes every worker's next `acquire` fail with `PoolClosed`, and
/// waits (up to `wait`) for checked-out connections to come back. The
/// TRUNCATE checkpoint then runs on the detached connection with nobody
/// else writing. Returns SQLite's `(busy, log_pages, checkpointed_pages)`.
///
// ponytail: workers are stopped at the DB, not individually. Each one's
// task is dropped at its next await when main exits. Give a worker its
// own cancellation token if it ever has to finish a unit of work
// (a file move) before exit.
pub async fn close_db(pool: &Pool, wait: Duration) -> Result<(i64, i64, i64), sqlx::Error> {
    let mut conn = tokio::time::timeout(wait, pool.acquire())
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)??
        .detach();
    if tokio::time::timeout(wait, pool.close()).await.is_err() {
        warn!(
            target: "longbox_web",
            wait_secs = wait.as_secs(),
            "a worker still held a DB connection at shutdown; checkpointing anyway"
        );
    }
    let result = sqlx::query_as::<_, (i64, i64, i64)>("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&mut conn)
        .await?;
    conn.close().await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

    #[tokio::test]
    async fn close_db_closes_the_pool_and_empties_the_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE t (x INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        for i in 0..200 {
            sqlx::query("INSERT INTO t VALUES (?)")
                .bind(i)
                .execute(&pool)
                .await
                .unwrap();
        }
        let wal = dir.path().join("t.db-wal");
        assert!(
            std::fs::metadata(&wal).unwrap().len() > 0,
            "setup: WAL has frames"
        );

        let (busy, _, _) = close_db(&pool, Duration::from_secs(2)).await.unwrap();
        assert_eq!(busy, 0);
        assert!(pool.is_closed(), "workers must be locked out");
        assert!(matches!(pool.acquire().await, Err(sqlx::Error::PoolClosed)));
        let wal_len = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert_eq!(wal_len, 0, "TRUNCATE checkpoint leaves an empty WAL");
    }
}
