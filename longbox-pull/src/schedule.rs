//! The daily scheduler task and the manual-trigger handle.
//!
//! A single Tokio task owns sweep execution, so scheduled and manual
//! sweeps can never overlap. The task waits on whichever comes first —
//! the next daily fire time or a manual trigger — runs one sweep, and
//! loops.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use time::{OffsetDateTime, Time};
use tokio::sync::mpsc;

use crate::config::PullConfig;
use crate::engine;

/// Handle to a running pull engine. Cheap to clone — the web layer
/// parks one in its shared state for the manual "Check now" route.
#[derive(Clone)]
pub struct PullHandle {
    /// Capacity-1 trigger channel. A queued trigger that the scheduler
    /// hasn't consumed coalesces a second request into the first.
    trigger: mpsc::Sender<()>,
    /// `true` while a sweep is executing.
    running: Arc<AtomicBool>,
}

impl PullHandle {
    /// Request an immediate sweep. Returns `false` when a sweep is
    /// already running (the web layer surfaces that as a 409) or the
    /// engine has shut down; `true` when the request was accepted.
    pub fn request_sweep(&self) -> bool {
        if self.running.load(Ordering::SeqCst) {
            return false;
        }
        // `Full` means a trigger is already queued — either way a sweep
        // is coming, so that still counts as accepted. Only a closed
        // channel (scheduler gone) is a real rejection.
        !matches!(
            self.trigger.try_send(()),
            Err(mpsc::error::TrySendError::Closed(())),
        )
    }

    /// Whether a sweep is executing right now.
    pub fn is_sweeping(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Spawn the scheduler task and return its handle.
pub fn spawn(config: PullConfig, db: longbox_db::Pool) -> PullHandle {
    let running = Arc::new(AtomicBool::new(false));
    let (trigger, rx) = mpsc::channel::<()>(1);
    let handle = PullHandle {
        trigger,
        running: Arc::clone(&running),
    };
    tokio::spawn(scheduler_loop(config, db, running, rx));
    handle
}

/// The scheduler loop: wait for the next daily slot or a manual
/// trigger, run one sweep, repeat. Exits when every [`PullHandle`] has
/// been dropped (the trigger channel closes) — i.e. at web-layer
/// shutdown.
async fn scheduler_loop(
    config: PullConfig,
    db: longbox_db::Pool,
    running: Arc<AtomicBool>,
    mut trigger: mpsc::Receiver<()>,
) {
    loop {
        let wait = duration_until_next(config.daily_time, OffsetDateTime::now_utc());
        tokio::select! {
            () = tokio::time::sleep(wait) => {
                tracing::info!(target: "longbox_pull", "pull.sweep_triggered (scheduled)");
            }
            msg = trigger.recv() => {
                if msg.is_none() {
                    // All handles dropped — no further triggers possible.
                    tracing::info!(target: "longbox_pull", "pull.scheduler_stopped");
                    return;
                }
                tracing::info!(target: "longbox_pull", "pull.sweep_triggered (manual)");
            }
        }

        running.store(true, Ordering::SeqCst);
        match engine::sweep(&db).await {
            Ok(s) => tracing::info!(
                target: "longbox_pull",
                polled = s.polled,
                submitted = s.submitted,
                no_match = s.no_match,
                submission_failed = s.submission_failed,
                grab_failed = s.grab_failed,
                indexer_errors = s.indexer_errors,
                "pull.sweep_complete"
            ),
            Err(e) => tracing::error!(
                target: "longbox_pull",
                error = %e,
                "pull.sweep_failed"
            ),
        }
        running.store(false, Ordering::SeqCst);
    }
}

/// Duration from `now` until the next occurrence of `daily_time`. When
/// today's slot has already passed, the next is tomorrow.
///
/// Pure wall-clock arithmetic in UTC (see [`PullConfig::daily_time`]).
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
    use time::macros::{datetime, time};

    #[test]
    fn next_fire_is_today_when_the_slot_is_still_ahead() {
        let now = datetime!(2026-05-20 03:00 UTC);
        let wait = duration_until_next(time!(05:00), now);
        assert_eq!(wait, Duration::from_secs(2 * 3600));
    }

    #[test]
    fn next_fire_rolls_to_tomorrow_once_the_slot_has_passed() {
        let now = datetime!(2026-05-20 06:00 UTC);
        let wait = duration_until_next(time!(05:00), now);
        assert_eq!(wait, Duration::from_secs(23 * 3600));
    }

    #[test]
    fn slot_exactly_now_waits_a_full_day() {
        let now = datetime!(2026-05-20 05:00 UTC);
        let wait = duration_until_next(time!(05:00), now);
        assert_eq!(wait, Duration::from_secs(24 * 3600));
    }

    #[test]
    fn request_sweep_is_rejected_while_a_sweep_runs() {
        let (trigger, _rx) = mpsc::channel::<()>(1);
        let running = Arc::new(AtomicBool::new(false));
        let handle = PullHandle {
            trigger,
            running: Arc::clone(&running),
        };
        // Idle — the request is accepted.
        assert!(handle.request_sweep());
        assert!(!handle.is_sweeping());
        // A sweep is running — a second request is rejected; the
        // `/pull/check` route surfaces that as a 409.
        running.store(true, Ordering::SeqCst);
        assert!(!handle.request_sweep());
        assert!(handle.is_sweeping());
    }
}
