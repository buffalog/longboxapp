//! The Metron coordinator. Every request from every caller goes through
//! one [`Gate`], shared by all lane handles of a `MetronClient`.
//!
//! Metron's published limits are 20 requests/minute (burst) and 5,000/day
//! (sustained); a 2026-10-14 calendar fan-out ran 20-wide at 60/min and
//! drew ~250 429s in a day. So:
//!
//! - **Concurrency 1.** One request in flight, held from send until the
//!   body is read.
//! - **Pacing.** Request starts are at least `min_interval` apart (4s =
//!   15/min, under the 20/min burst).
//! - **Daily budget.** `daily_budget` requests per UTC day (4,000, under
//!   5,000). Seeded from Metron's sustained headers, so a restart doesn't
//!   reset what Metron has already counted.
//! - **Headers.** `X-RateLimit-{Burst,Sustained}-{Remaining,Reset}` are
//!   read on every response; when either window is down to
//!   [`LOW_REMAINING`], everyone waits until that window's reset.
//! - **429.** Pauses ALL callers until `Retry-After` plus jitter. With
//!   concurrency 1 the retry is serial, never a herd.
//! - **Lanes.** Background callers (the linkers, the finished sweep) yield
//!   to interactive ones (the calendar): they don't take the slot while
//!   an interactive request is waiting for it.
//!
//! Pacing, pauses and header waits all fold into one `not_before` instant.
//! Only the permit holder moves it, so there's no race between them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::HeaderMap;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::time::Instant;
use tracing::info;

use crate::error::MetronError;

/// Who is asking. Picks the yield behavior and how long a pause is
/// waited out before failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// A user is waiting (the forward calendar, a calendar subscribe).
    /// Takes precedence over background; fails fast on a long pause.
    Interactive,
    /// Linkers and sweeps. Yields to interactive; waits out any pause.
    Background,
}

/// A window with this many requests left (or fewer) is treated as spent.
pub(crate) const LOW_REMAINING: u64 = 1;
/// An interactive caller facing a longer pause than this gets
/// `RateLimited` instead of a hung page.
const INTERACTIVE_MAX_PAUSE: Duration = Duration::from_secs(120);
/// How often a yielding background caller re-checks for interactive waiters.
const YIELD_POLL: Duration = Duration::from_millis(250);

pub(crate) struct Gate {
    permit: Semaphore,
    interactive_waiting: AtomicUsize,
    state: Mutex<State>,
    min_interval: Duration,
    daily_budget: u32,
    jitter_max: Duration,
}

struct State {
    not_before: Instant,
    day: u64,
    used: u32,
    interactive: u32,
    background: u32,
    rate_limited: u32,
}

/// Decrements the interactive-waiting count even when the waiting future
/// is dropped (an axum handler cancelled by a closed tab). A leaked count
/// would park the background lane forever.
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Gate {
    pub(crate) fn new(min_interval: Duration, daily_budget: u32, jitter_max: Duration) -> Self {
        Self {
            permit: Semaphore::new(1),
            interactive_waiting: AtomicUsize::new(0),
            state: Mutex::new(State {
                not_before: Instant::now(),
                day: utc_day(),
                used: 0,
                interactive: 0,
                background: 0,
                rate_limited: 0,
            }),
            min_interval,
            daily_budget,
            jitter_max,
        }
    }

    /// Wait for the slot, then for `not_before`, then spend one request
    /// of the daily budget. Hold the returned permit until the response
    /// body is read.
    pub(crate) async fn acquire(&self, lane: Lane) -> Result<SemaphorePermit<'_>, MetronError> {
        let permit = match lane {
            Lane::Interactive => {
                self.interactive_waiting.fetch_add(1, Ordering::SeqCst);
                let _waiting = Waiting(&self.interactive_waiting);
                self.permit.acquire().await
            }
            Lane::Background => loop {
                while self.interactive_waiting.load(Ordering::SeqCst) > 0 {
                    tokio::time::sleep(YIELD_POLL).await;
                }
                let p = self.permit.acquire().await;
                // An interactive caller may have queued while we waited.
                if self.interactive_waiting.load(Ordering::SeqCst) == 0 {
                    break p;
                }
            },
        }
        .expect("the gate's semaphore is never closed");

        let wait = self
            .lock()
            .not_before
            .saturating_duration_since(Instant::now());
        if lane == Lane::Interactive && wait > INTERACTIVE_MAX_PAUSE {
            return Err(MetronError::RateLimited {
                retry_after_seconds: wait.as_secs().saturating_add(1),
            });
        }
        tokio::time::sleep(wait).await;

        let mut s = self.lock();
        s.roll_day(utc_day());
        if s.used >= self.daily_budget {
            return Err(MetronError::RateLimited {
                retry_after_seconds: secs_to_utc_midnight(),
            });
        }
        s.used += 1;
        match lane {
            Lane::Interactive => s.interactive += 1,
            Lane::Background => s.background += 1,
        }
        s.not_before = Instant::now() + self.min_interval;
        Ok(permit)
    }

    /// Read Metron's rate-limit headers off any response (429s included).
    pub(crate) fn note_headers(&self, headers: &HeaderMap) {
        let now_unix = unix_now();
        let mut s = self.lock();
        for window in ["burst", "sustained"] {
            let remaining = header_u64(headers, &format!("x-ratelimit-{window}-remaining"));
            let reset = header_u64(headers, &format!("x-ratelimit-{window}-reset"));
            if let (Some(remaining), Some(reset)) = (remaining, reset) {
                if remaining <= LOW_REMAINING {
                    // +1s: don't land on the reset second itself.
                    let until = Instant::now()
                        + Duration::from_secs(reset.saturating_sub(now_unix).saturating_add(1));
                    s.not_before = s.not_before.max(until);
                }
            }
        }
        // Metron's own count of today's requests beats ours after a restart.
        let limit = header_u64(headers, "x-ratelimit-sustained-limit");
        let remaining = header_u64(headers, "x-ratelimit-sustained-remaining");
        if let (Some(limit), Some(remaining)) = (limit, remaining) {
            let spent = u32::try_from(limit.saturating_sub(remaining)).unwrap_or(u32::MAX);
            s.used = s.used.max(spent);
        }
    }

    /// A 429: pause every caller for `retry_after` plus jitter. Returns
    /// the pause applied.
    pub(crate) fn note_429(&self, retry_after: Duration) -> Duration {
        let pause = retry_after + jitter(self.jitter_max);
        let mut s = self.lock();
        s.rate_limited += 1;
        s.not_before = s.not_before.max(Instant::now() + pause);
        pause
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic while holding this lock leaves plain counters behind;
        // nothing to repair, keep going.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl State {
    /// On the first request of a new UTC day, log yesterday's usage
    /// (the per-caller count the 2026-10-14 incident had no record of)
    /// and reset the budget.
    fn roll_day(&mut self, today: u64) {
        if today == self.day {
            return;
        }
        if self.used > 0 || self.rate_limited > 0 {
            info!(
                target: "longbox_metron",
                used = self.used,
                interactive = self.interactive,
                background = self.background,
                rate_limited = self.rate_limited,
                "metron daily usage"
            );
        }
        self.day = today;
        self.used = 0;
        self.interactive = 0;
        self.background = 0;
        self.rate_limited = 0;
    }
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn utc_day() -> u64 {
    unix_now() / 86_400
}

fn secs_to_utc_midnight() -> u64 {
    86_400 - unix_now() % 86_400
}

// ponytail: clock-nanos jitter, not a PRNG. It only has to de-sync us
// from Metron's window edge, and concurrency 1 already rules out a herd.
fn jitter(max: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u128::from(d.subsec_nanos()))
        .unwrap_or(0);
    Duration::from_nanos(u64::try_from(max.as_nanos() * nanos / 1_000_000_000).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use std::sync::Arc;

    fn gate(min_interval_secs: u64, budget: u32) -> Gate {
        Gate::new(
            Duration::from_secs(min_interval_secs),
            budget,
            Duration::ZERO,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn request_starts_are_paced_min_interval_apart() {
        let g = gate(4, 100);
        let t0 = Instant::now();
        for _ in 0..3 {
            drop(g.acquire(Lane::Interactive).await.unwrap());
        }
        // Starts at 0s, 4s, 8s: 15/min.
        assert_eq!(t0.elapsed(), Duration::from_secs(8));
    }

    #[tokio::test(start_paused = true)]
    async fn a_429_pauses_every_caller_on_both_lanes() {
        let g = Arc::new(gate(0, 100));
        drop(g.acquire(Lane::Interactive).await.unwrap());
        assert_eq!(g.note_429(Duration::from_secs(30)), Duration::from_secs(30));

        let t0 = Instant::now();
        let bg = {
            let g = Arc::clone(&g);
            tokio::spawn(async move { drop(g.acquire(Lane::Background).await.unwrap()) })
        };
        drop(g.acquire(Lane::Interactive).await.unwrap());
        assert!(
            t0.elapsed() >= Duration::from_secs(30),
            "interactive went early"
        );
        bg.await.unwrap();
        assert!(
            t0.elapsed() >= Duration::from_secs(30),
            "background went early"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn interactive_fails_fast_on_a_long_pause_background_waits_it_out() {
        let g = gate(0, 100);
        g.note_429(Duration::from_secs(600));
        let t0 = Instant::now();
        assert!(matches!(
            g.acquire(Lane::Interactive).await,
            Err(MetronError::RateLimited { .. })
        ));
        assert_eq!(t0.elapsed(), Duration::ZERO);
        drop(g.acquire(Lane::Background).await.unwrap());
        assert!(t0.elapsed() >= Duration::from_secs(600));
    }

    #[tokio::test(start_paused = true)]
    async fn low_remaining_waits_until_the_window_resets() {
        let g = gate(0, 100);
        let mut h = HeaderMap::new();
        h.insert(
            "x-ratelimit-burst-remaining",
            HeaderValue::from(LOW_REMAINING),
        );
        h.insert(
            "x-ratelimit-burst-reset",
            HeaderValue::from(unix_now() + 20),
        );
        g.note_headers(&h);
        let t0 = Instant::now();
        drop(g.acquire(Lane::Background).await.unwrap());
        assert!(t0.elapsed() >= Duration::from_secs(20));

        // Plenty left: no wait.
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-burst-remaining", HeaderValue::from(15u64));
        h.insert(
            "x-ratelimit-burst-reset",
            HeaderValue::from(unix_now() + 20),
        );
        g.note_headers(&h);
        let t1 = Instant::now();
        drop(g.acquire(Lane::Background).await.unwrap());
        assert_eq!(t1.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn daily_budget_stops_requests_and_is_seeded_from_metrons_count() {
        let g = gate(0, 2);
        drop(g.acquire(Lane::Interactive).await.unwrap());
        drop(g.acquire(Lane::Background).await.unwrap());
        assert!(matches!(
            g.acquire(Lane::Interactive).await,
            Err(MetronError::RateLimited { .. })
        ));

        // After a restart our count is 0, but Metron says 3,999 of 5,000
        // are gone: a 4,000 budget has one left.
        let g = gate(0, 4_000);
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-sustained-limit", HeaderValue::from(5_000u64));
        h.insert(
            "x-ratelimit-sustained-remaining",
            HeaderValue::from(1_001u64),
        );
        g.note_headers(&h);
        drop(g.acquire(Lane::Interactive).await.unwrap());
        assert!(g.acquire(Lane::Interactive).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn background_yields_the_slot_to_a_waiting_interactive_caller() {
        let g = Arc::new(gate(4, 100));
        let order = Arc::new(Mutex::new(Vec::new()));
        // Hold the slot so both of the others queue.
        let held = g.acquire(Lane::Interactive).await.unwrap();

        let spawn = |lane: Lane, name: &'static str| {
            let (g, order) = (Arc::clone(&g), Arc::clone(&order));
            tokio::spawn(async move {
                drop(g.acquire(lane).await.unwrap());
                order.lock().unwrap().push(name);
            })
        };
        // Background queues first, interactive second.
        let bg = spawn(Lane::Background, "background");
        tokio::time::sleep(Duration::from_millis(10)).await;
        let it = spawn(Lane::Interactive, "interactive");
        tokio::time::sleep(Duration::from_millis(10)).await;

        drop(held);
        it.await.unwrap();
        bg.await.unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["interactive", "background"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_interactive_wait_does_not_strand_the_background_lane() {
        let g = Arc::new(gate(0, 100));
        let held = g.acquire(Lane::Background).await.unwrap();
        let cancelled = {
            let g = Arc::clone(&g);
            tokio::spawn(async move {
                let _ = g.acquire(Lane::Interactive).await;
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancelled.abort();
        let _ = cancelled.await;
        drop(held);
        let _slot = tokio::time::timeout(Duration::from_secs(5), g.acquire(Lane::Background))
            .await
            .expect("background lane stranded by a leaked waiter")
            .unwrap();
    }
}
