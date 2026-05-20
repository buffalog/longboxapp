//! LongBox scheduled-scan timer.
//!
//! A generic daily scheduler: [`start`] spawns a tokio task that, once
//! per day at a configured wall-clock time, awaits a caller-supplied
//! async closure. LongBox uses it to run the daily full library scan
//! ahead of the pull engine's sweep.
//!
//! The crate is deliberately scan-agnostic. A scan needs `longbox-web`'s
//! `scan_status` types, and depending on `longbox-web` would be a
//! dependency cycle — so `bootstrap.rs` builds the scan closure (the
//! `scan_status`-guarded full scan) and hands it here. Boot shape and
//! the `duration_until_next` arithmetic mirror `longbox-pull`'s
//! scheduler.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use time::{OffsetDateTime, Time};

/// Scheduled-scan configuration, built by the web layer from env vars.
#[derive(Debug, Clone)]
pub struct ScanSchedulerConfig {
    /// Wall-clock time the daily scan fires. Interpreted as **UTC** —
    /// `OffsetDateTime::now_local()` is unreliable inside a
    /// multi-threaded Tokio process, the same rationale as
    /// `longbox_pull::PullConfig::daily_time`.
    pub daily_time: Time,
}

impl Default for ScanSchedulerConfig {
    fn default() -> Self {
        Self {
            daily_time: time::macros::time!(03:00),
        }
    }
}

/// Handle to a running scan scheduler. Cheap to clone — the web layer
/// parks one in its shared state. Scans keep their own manual trigger
/// (the scan route), so this handle exposes only a status read.
#[derive(Clone)]
pub struct ScanSchedulerHandle {
    is_scanning: Arc<AtomicBool>,
}

impl ScanSchedulerHandle {
    /// Whether a *scheduled* scan is executing right now.
    pub fn is_scanning(&self) -> bool {
        self.is_scanning.load(Ordering::SeqCst)
    }
}

/// Start the scan scheduler: spawn the daily timer task and return its
/// handle. `scan_fn` is awaited once per day at `config.daily_time`; it
/// is expected to handle its own errors — the scheduler only records
/// that a fire happened.
pub fn start<F, Fut>(config: ScanSchedulerConfig, scan_fn: F) -> ScanSchedulerHandle
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let is_scanning = Arc::new(AtomicBool::new(false));
    let handle = ScanSchedulerHandle {
        is_scanning: Arc::clone(&is_scanning),
    };
    tokio::spawn(scheduler_loop(config, scan_fn, is_scanning));
    handle
}

/// Sleep until the next daily slot, run the scan closure, repeat.
async fn scheduler_loop<F, Fut>(
    config: ScanSchedulerConfig,
    scan_fn: F,
    is_scanning: Arc<AtomicBool>,
) where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    loop {
        let wait = duration_until_next(config.daily_time, OffsetDateTime::now_utc());
        tokio::time::sleep(wait).await;
        tracing::info!(target: "longbox_scan_scheduler", "scan.scheduled_fire");
        is_scanning.store(true, Ordering::SeqCst);
        scan_fn().await;
        is_scanning.store(false, Ordering::SeqCst);
    }
}

/// Duration from `now` until the next occurrence of `daily_time`. When
/// today's slot has already passed, the next is tomorrow. Pure
/// wall-clock arithmetic in UTC.
fn duration_until_next(daily_time: Time, now: OffsetDateTime) -> Duration {
    let today_fire = now.replace_time(daily_time);
    let next_fire = if today_fire > now {
        today_fire
    } else {
        today_fire.saturating_add(time::Duration::days(1))
    };
    // next_fire > now by construction, so the conversion never fails.
    (next_fire - now).try_into().unwrap_or(Duration::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use time::macros::{datetime, time};

    #[test]
    fn next_fire_is_today_when_the_slot_is_still_ahead() {
        let now = datetime!(2026-05-20 01:00 UTC);
        assert_eq!(
            duration_until_next(time!(03:00), now),
            Duration::from_secs(2 * 3600)
        );
    }

    #[test]
    fn next_fire_rolls_to_tomorrow_once_the_slot_has_passed() {
        let now = datetime!(2026-05-20 04:00 UTC);
        assert_eq!(
            duration_until_next(time!(03:00), now),
            Duration::from_secs(23 * 3600)
        );
    }

    #[test]
    fn slot_exactly_now_waits_a_full_day() {
        let now = datetime!(2026-05-20 03:00 UTC);
        assert_eq!(
            duration_until_next(time!(03:00), now),
            Duration::from_secs(24 * 3600)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn scheduler_fires_the_scan_closure_after_its_daily_slot() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_fn = Arc::clone(&calls);
        let handle = start(ScanSchedulerConfig::default(), move || {
            let calls = Arc::clone(&calls_for_fn);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert!(!handle.is_scanning(), "idle before the first fire");

        // Let the spawned scheduler task run far enough to register its
        // first `sleep` timer before time is advanced past it.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        // Fast-forward past the next 03:00 UTC slot (at most ~24h away).
        tokio::time::advance(Duration::from_secs(25 * 3600)).await;
        // Let the woken scheduler task run the closure.
        for _ in 0..32 {
            if calls.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "the scheduler ran the scan closure once its daily slot elapsed"
        );
    }
}
