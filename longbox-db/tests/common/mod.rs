//! Shared test helpers. Each integration-test binary `mod common;`s this.

use longbox_db::pool;
use sqlx::SqlitePool;
use time::macros::datetime;
use time::PrimitiveDateTime;

#[allow(dead_code)]
pub async fn fresh_pool() -> SqlitePool {
    pool::open(":memory:")
        .await
        .expect("open in-memory pool with migrations applied")
}

/// Stable test timestamp so equality assertions don't have to mock `now`.
#[allow(dead_code)]
pub fn fixed_ts() -> PrimitiveDateTime {
    datetime!(2026-01-15 12:34:56)
}
