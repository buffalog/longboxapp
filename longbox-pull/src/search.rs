//! On-demand single-series search — the manual "Search now" trigger and
//! the auto-fire-on-subscribe hook.
//!
//! Independent of the daily scheduler ([`crate::schedule`]). The daily
//! sweep walks every subscribed series; on-demand searches walk one.
//! Concurrency model is per-series: two searches for the SAME series
//! return 409, two for DIFFERENT series run in parallel. A daily sweep
//! running concurrently with an on-demand search is NOT blocked — the
//! documented race surface is at [`engine::sweep_single_series`].

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::engine;
use longbox_db::Pool;

/// Tracker of in-progress on-demand searches, keyed by `series_id`.
/// Cheap to clone — the web layer parks one in shared state for the
/// route handler and the auto-trigger sites.
#[derive(Clone)]
pub struct PullSearchHandle {
    in_progress: Arc<Mutex<HashSet<i64>>>,
    db: Pool,
}

impl PullSearchHandle {
    /// Construct a handle bound to a pool. The handle spawns its own
    /// short-lived tokio tasks per search; the pool clone is what they
    /// run against.
    pub fn new(db: Pool) -> Self {
        Self {
            in_progress: Arc::new(Mutex::new(HashSet::new())),
            db,
        }
    }

    /// Atomically mark `series_id` as in-progress and spawn its search.
    /// Returns `true` when the search was accepted, `false` when one is
    /// already running for that series. The web layer surfaces `false`
    /// as a 409. Auto-trigger callers ignore the return value — a
    /// duplicate fire is a silent no-op, which is exactly what we want
    /// when the same series shows up twice in a bulk-add payload.
    pub async fn try_start(&self, series_id: i64) -> bool {
        {
            let mut set = self.in_progress.lock().await;
            if !set.insert(series_id) {
                return false;
            }
        }
        let in_progress = Arc::clone(&self.in_progress);
        let db = self.db.clone();
        tokio::spawn(async move {
            let outcome = engine::sweep_single_series(&db, series_id).await;
            match &outcome {
                Ok(summary) => tracing::info!(
                    target: "longbox_pull",
                    series_id,
                    polled = summary.polled,
                    submitted = summary.submitted,
                    no_match = summary.no_match,
                    submission_failed = summary.submission_failed,
                    series_mismatched = summary.series_mismatched,
                    indexer_errors = summary.indexer_errors,
                    "pull.search_complete"
                ),
                Err(e) => tracing::warn!(
                    target: "longbox_pull",
                    series_id,
                    err = %e,
                    "pull.search_failed"
                ),
            }
            in_progress.lock().await.remove(&series_id);
        });
        true
    }

    /// Whether an on-demand search is in flight for `series_id`. Used
    /// by tests; the frontend has no live feed and instead uses a
    /// timed disabled state on the button (15 s, chosen to cover the
    /// typical indexer-search wall time without locking the button
    /// indefinitely).
    pub async fn is_searching(&self, series_id: i64) -> bool {
        self.in_progress.lock().await.contains(&series_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_pool() -> Pool {
        longbox_db::open(":memory:").await.unwrap()
    }

    #[tokio::test]
    async fn try_start_returns_true_first_call_false_second() {
        let handle = PullSearchHandle::new(fresh_pool().await);
        // Reserve the slot directly so we test the guard without
        // racing the spawned task to remove the entry. With no
        // downloader configured the spawned engine call returns
        // immediately, so the second call would race.
        handle.in_progress.lock().await.insert(42);
        let second = handle.try_start(42).await;
        assert!(!second, "duplicate same-series search must be rejected");
        assert!(handle.is_searching(42).await);
    }

    #[tokio::test]
    async fn try_start_independent_across_distinct_series() {
        let handle = PullSearchHandle::new(fresh_pool().await);
        // Two different series accepted independently — pre-seed both
        // slots so we don't race the spawned task that would clear
        // them on completion (engine returns immediately on no
        // downloader).
        handle.in_progress.lock().await.insert(1);
        handle.in_progress.lock().await.insert(2);
        assert!(handle.is_searching(1).await);
        assert!(handle.is_searching(2).await);
        // A third, distinct series is still accepted; the guard is
        // per-series, not global.
        assert!(handle.try_start(3).await);
    }
}
