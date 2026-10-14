//! Process-wide ComicVine traffic control. Every CV request goes through
//! one [`CvCoordinator`], whichever client handle sends it. The
//! coordinator owns:
//!
//! - one token bucket per lane, so background work (enrichment, credits,
//!   the full catalog refresh) can't drain the budget user-initiated
//!   requests wait on;
//! - a process-wide minimum spacing between requests, because ComicVine
//!   also runs per-second "velocity detection" on top of its hourly
//!   per-resource cap, and a lane burst must not turn into a
//!   same-second burst on the wire;
//! - background *yield*: every interactive request stamps
//!   `last_interactive`, and background requests hold off until
//!   `background_yield` has passed since the last one, so a user adding
//!   series back to back runs at interactive speed;
//! - the shared *429 pause*: a real 429 from ComicVine means our budget
//!   math is wrong, so both lanes stop for its `Retry-After`;
//! - the "full refresh running" flag the credits resolver defers to;
//! - the per-lane call counters behind the rate-limit chip.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::error::CvError;
use crate::rate_limit::CvRateLimiter;

/// Which budget a request spends, and how it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// User-initiated routes. Interactive bucket; stamps user activity. A
    /// request that can't go out within `interactive_max_wait` (no token,
    /// or a 429 pause longer than that) fails fast with
    /// [`CvError::RateLimited`] instead of hanging the page.
    Interactive,
    /// Enrichment, the credits resolver, the full catalog refresh and the
    /// calendar's bulk-subscribe resolves. Background bucket; yields to
    /// recent interactive activity; waits as long as it takes.
    Background,
    /// The delta refresh at the top of each pull sweep. Spends the
    /// *interactive* bucket, because its ~10 calendar pages would take
    /// ~7 minutes at the background rate. Does not stamp user activity and
    /// does not yield to it (a sweep's ~10 calls shouldn't stall behind a
    /// user adding series), honors the 429 pause, and waits like
    /// background (never fails a sweep with `RateLimited`).
    Delta,
}

/// Lane budgets. Read from settings at boot; a change needs a restart.
#[derive(Debug, Clone)]
pub struct CvLanesConfig {
    pub interactive_rate_per_hour: u32,
    pub interactive_burst: u32,
    /// Must exceed one token period (3600 / rate seconds), or a request
    /// arriving after the burst is spent can never get a token in time.
    /// [`Self::interactive_max_wait_for`] derives it.
    pub interactive_max_wait: Duration,
    pub background_rate_per_hour: u32,
    /// How long background requests hold off after an interactive one.
    pub background_yield: Duration,
    /// Minimum gap between any two CV requests, either lane.
    pub min_spacing: Duration,
}

impl Default for CvLanesConfig {
    /// 100 + 80 = 180/h, the old single bucket: the split can't be worse
    /// than before against ComicVine's 200-per-resource-per-hour limit.
    fn default() -> Self {
        Self {
            interactive_rate_per_hour: 100,
            interactive_burst: 10,
            interactive_max_wait: Self::interactive_max_wait_for(100),
            background_rate_per_hour: 80,
            background_yield: Duration::from_secs(300),
            min_spacing: Duration::from_secs(1),
        }
    }
}

impl CvLanesConfig {
    /// One token period plus 10s: the next token is always reachable,
    /// while a long 429 pause still fails fast. 46s at 100/h.
    pub fn interactive_max_wait_for(rate_per_hour: u32) -> Duration {
        Duration::from_secs(3600 / u64::from(rate_per_hour.max(1)) + 10)
    }
}

/// Background requests come in ones (each consumer is sequential); a small
/// burst just lets a quiet hour's saved tokens drain at the pacer's rate.
const BACKGROUND_BURST: u32 = 5;
/// "Wait as long as it takes" for the background bucket and the delta.
/// One token is at most 45s away at any sane rate; an hour is a backstop.
const PATIENT_WAIT: Duration = Duration::from_secs(3600);

/// Snapshot of one lane's per-hour call counter — what the rate-limit chip
/// shows. `count` is requests sent since `window_started_at_unix`.
#[derive(Debug, Clone, Copy)]
pub struct CvRateLimitSnapshot {
    pub count: u32,
    pub limit_per_hour: u32,
    pub window_started_at_unix: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct CvLanesSnapshot {
    /// Includes the delta's requests: they spend the interactive bucket.
    pub interactive: CvRateLimitSnapshot,
    pub background: CvRateLimitSnapshot,
    /// Time left before background work may resume after the last
    /// interactive request (zero when it isn't yielding).
    pub yielding_for: Duration,
    /// Time left on a 429 pause (zero when not paused).
    pub paused_for: Duration,
}

pub struct CvCoordinator {
    config: CvLanesConfig,
    interactive: CvRateLimiter,
    background: CvRateLimiter,
    interactive_calls: CallCounter,
    background_calls: CallCounter,
    gates: Mutex<Gates>,
    full_refresh_running: AtomicBool,
}

#[derive(Default)]
struct Gates {
    /// Earliest instant the next request may go out (the pacer).
    next_slot: Option<Instant>,
    last_interactive: Option<Instant>,
    paused_until: Option<Instant>,
}

impl std::fmt::Debug for CvCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CvCoordinator")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl CvCoordinator {
    pub fn new(config: CvLanesConfig) -> Self {
        Self {
            interactive: CvRateLimiter::new(
                config.interactive_rate_per_hour,
                config.interactive_burst,
            ),
            background: CvRateLimiter::new(config.background_rate_per_hour, BACKGROUND_BURST),
            interactive_calls: CallCounter::new(),
            background_calls: CallCounter::new(),
            gates: Mutex::default(),
            full_refresh_running: AtomicBool::new(false),
            config,
        }
    }

    /// Wait until `lane` may send one request, then count it.
    pub async fn acquire(&self, lane: Lane) -> Result<(), CvError> {
        if lane == Lane::Interactive {
            // Stamp before waiting: background yields from the moment the
            // user acts, not from when their request finally goes out.
            self.gates().last_interactive = Some(Instant::now());
        }
        match lane {
            Lane::Interactive => {
                self.interactive
                    .acquire(self.config.interactive_max_wait)
                    .await?
            }
            Lane::Delta => self.interactive.acquire(PATIENT_WAIT).await?,
            Lane::Background => self.background.acquire(PATIENT_WAIT).await?,
        }
        loop {
            let wait = self.gate_wait(lane);
            if wait.is_zero() {
                break;
            }
            if lane == Lane::Interactive && wait > self.config.interactive_max_wait {
                return Err(CvError::RateLimited {
                    retry_after_seconds: ceil_secs(wait),
                });
            }
            // Re-check after sleeping: a new 429 or interactive stamp may
            // have pushed the gate further out.
            tokio::time::sleep(wait).await;
        }
        self.pace().await;
        match lane {
            Lane::Interactive | Lane::Delta => self.interactive_calls.consume(),
            Lane::Background => self.background_calls.consume(),
        }
        Ok(())
    }

    /// How long `lane` must still hold off: the 429 pause for everyone,
    /// plus the yield window for [`Lane::Background`].
    fn gate_wait(&self, lane: Lane) -> Duration {
        let gates = self.gates();
        let now = Instant::now();
        let paused = gates
            .paused_until
            .map_or(Duration::ZERO, |until| until.saturating_duration_since(now));
        let yielding = match (lane, gates.last_interactive) {
            (Lane::Background, Some(at)) => {
                (at + self.config.background_yield).saturating_duration_since(now)
            }
            _ => Duration::ZERO,
        };
        paused.max(yielding)
    }

    /// Stop both lanes for `retry_after` — called on a real 429. Never
    /// shortens a pause already in force.
    pub fn pause(&self, retry_after: Duration) {
        let until = Instant::now() + retry_after;
        let mut gates = self.gates();
        gates.paused_until = Some(gates.paused_until.map_or(until, |p| p.max(until)));
    }

    pub fn set_full_refresh_running(&self, running: bool) {
        self.full_refresh_running.store(running, Ordering::SeqCst);
    }

    /// The credits resolver holds off while this is set: credits are
    /// cosmetic, the refresh gets comics on disk.
    pub fn full_refresh_running(&self) -> bool {
        self.full_refresh_running.load(Ordering::SeqCst)
    }

    /// Reserve the next send slot, `min_spacing` after the previous one,
    /// and sleep until it.
    async fn pace(&self) {
        let slot = {
            let mut gates = self.gates();
            let now = Instant::now();
            let slot = gates.next_slot.map_or(now, |n| n.max(now));
            gates.next_slot = Some(slot + self.config.min_spacing);
            slot
        };
        tokio::time::sleep_until(slot).await;
    }

    fn gates(&self) -> std::sync::MutexGuard<'_, Gates> {
        self.gates.lock().expect("CV coordinator mutex poisoned")
    }

    pub fn snapshot(&self) -> CvLanesSnapshot {
        let (yielding_for, paused_for) = {
            let gates = self.gates();
            let now = Instant::now();
            let left = |at: Option<Instant>| {
                at.map_or(Duration::ZERO, |t| t.saturating_duration_since(now))
            };
            (
                left(
                    gates
                        .last_interactive
                        .map(|at| at + self.config.background_yield),
                ),
                left(gates.paused_until),
            )
        };
        CvLanesSnapshot {
            yielding_for,
            paused_for,
            interactive: self
                .interactive_calls
                .snapshot(self.config.interactive_rate_per_hour),
            background: self
                .background_calls
                .snapshot(self.config.background_rate_per_hour),
        }
    }
}

fn ceil_secs(d: Duration) -> u64 {
    d.as_secs().saturating_add(u64::from(d.subsec_nanos() > 0))
}

/// Sliding-window counter for "calls made in the last hour-ish."
/// Pragmatic implementation: a single atomic counter + a window-start
/// timestamp. Rolls over when the window expires on the next
/// `consume` call. Not strictly a 60-minute trailing window — closer
/// to "since the last roll-over" — but plenty accurate for a UI
/// chip.
struct CallCounter {
    count: AtomicU32,
    window_started_at: AtomicI64,
}

impl CallCounter {
    const WINDOW_SECS: i64 = 3600;

    fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            window_started_at: AtomicI64::new(0),
        }
    }

    fn now_unix_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }

    /// Increment, rolling the window if it has expired. The atomic
    /// ordering is loose because the counter is advisory — strict
    /// linearizability between concurrent increments isn't worth the
    /// fence overhead for a UI display.
    fn consume(&self) {
        let now = Self::now_unix_secs();
        let start = self.window_started_at.load(Ordering::Acquire);
        if start == 0 || now - start >= Self::WINDOW_SECS {
            self.window_started_at.store(now, Ordering::Release);
            self.count.store(1, Ordering::Release);
        } else {
            self.count.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn snapshot(&self, limit_per_hour: u32) -> CvRateLimitSnapshot {
        let now = Self::now_unix_secs();
        let start = self.window_started_at.load(Ordering::Acquire);
        let (count, window_started_at_unix) = if start == 0 || now - start >= Self::WINDOW_SECS {
            (0, now)
        } else {
            (self.count.load(Ordering::Acquire), start)
        };
        CvRateLimitSnapshot {
            count,
            limit_per_hour,
            window_started_at_unix,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Huge buckets, no pacing, no yield: tests opt into one mechanism at a
    /// time. (Governor reads the real clock, so tests that run under
    /// tokio's paused clock must never actually wait on a bucket.)
    fn open() -> CvLanesConfig {
        CvLanesConfig {
            interactive_rate_per_hour: 360_000,
            interactive_burst: 1_000,
            interactive_max_wait: Duration::from_secs(10),
            background_rate_per_hour: 360_000,
            background_yield: Duration::ZERO,
            min_spacing: Duration::ZERO,
        }
    }

    /// Two interactive tokens, `BACKGROUND_BURST` background ones, ~no refill.
    fn tight() -> CvCoordinator {
        CvCoordinator::new(CvLanesConfig {
            interactive_rate_per_hour: 1,
            interactive_burst: 2,
            interactive_max_wait: Duration::from_millis(50),
            background_rate_per_hour: 1,
            ..open()
        })
    }

    async fn blocks(c: &CvCoordinator, lane: Lane) -> bool {
        tokio::time::timeout(Duration::from_millis(100), c.acquire(lane))
            .await
            .is_err()
    }

    #[tokio::test]
    async fn a_drained_background_lane_does_not_slow_interactive() {
        let c = tight();
        for _ in 0..BACKGROUND_BURST {
            c.acquire(Lane::Background).await.unwrap();
        }
        assert!(
            blocks(&c, Lane::Background).await,
            "background bucket should be empty"
        );

        c.acquire(Lane::Interactive).await.unwrap();
        c.acquire(Lane::Interactive).await.unwrap();
        let err = c.acquire(Lane::Interactive).await.unwrap_err();
        assert!(matches!(err, CvError::RateLimited { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn delta_spends_the_interactive_bucket_not_the_background_one() {
        let c = tight();
        c.acquire(Lane::Background).await.unwrap();
        c.acquire(Lane::Background).await.unwrap();
        c.acquire(Lane::Delta).await.unwrap();
        c.acquire(Lane::Delta).await.unwrap();
        // Interactive is now empty too, and delta waits rather than failing.
        assert!(blocks(&c, Lane::Delta).await);
        assert!(c.acquire(Lane::Interactive).await.is_err());
        assert_eq!(c.snapshot().interactive.count, 2);
        assert_eq!(c.snapshot().background.count, 2);
    }

    /// The burst is spent; the next request must still get a token. A max
    /// wait shorter than one token period fails this every time.
    #[tokio::test]
    async fn an_interactive_request_after_the_burst_still_gets_a_token() {
        for rate in [1, 80, 100, 180, 3600] {
            assert!(
                CvLanesConfig::interactive_max_wait_for(rate)
                    > Duration::from_secs(3600 / u64::from(rate)),
                "rate {rate}"
            );
        }
        let c = CvCoordinator::new(CvLanesConfig {
            interactive_rate_per_hour: 3600, // one token a second
            interactive_burst: 2,
            interactive_max_wait: CvLanesConfig::interactive_max_wait_for(3600),
            ..open()
        });
        for _ in 0..3 {
            c.acquire(Lane::Interactive).await.unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn background_yields_to_interactive_activity_but_delta_does_not() {
        let c = CvCoordinator::new(CvLanesConfig {
            background_yield: Duration::from_secs(300),
            ..open()
        });

        // Delta is not user activity: background isn't held off by it.
        c.acquire(Lane::Delta).await.unwrap();
        let t = Instant::now();
        c.acquire(Lane::Background).await.unwrap();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());

        c.acquire(Lane::Interactive).await.unwrap();
        assert!(c.snapshot().yielding_for > Duration::from_secs(299));

        let t = Instant::now();
        c.acquire(Lane::Delta).await.unwrap();
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "delta yielded: {:?}",
            t.elapsed()
        );

        let t = Instant::now();
        c.acquire(Lane::Background).await.unwrap();
        assert!(t.elapsed() >= Duration::from_secs(299), "{:?}", t.elapsed());
        assert_eq!(c.snapshot().yielding_for, Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_429_pause_holds_every_lane() {
        let c = CvCoordinator::new(open());
        c.pause(Duration::from_secs(30));
        // A shorter pause never shortens one in force.
        c.pause(Duration::from_secs(5));
        assert!(c.snapshot().paused_for > Duration::from_secs(29));

        // Longer than interactive's 10s max wait: fail fast with the real wait.
        match c.acquire(Lane::Interactive).await.unwrap_err() {
            CvError::RateLimited {
                retry_after_seconds,
            } => assert_eq!(retry_after_seconds, 30),
            other => panic!("expected RateLimited, got {other:?}"),
        }

        let t = Instant::now();
        c.acquire(Lane::Delta).await.unwrap();
        assert!(t.elapsed() >= Duration::from_secs(29), "{:?}", t.elapsed());
        // The pause is over; background goes straight out.
        let t = Instant::now();
        c.acquire(Lane::Background).await.unwrap();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn the_pacer_spaces_requests_across_lanes() {
        let c = CvCoordinator::new(CvLanesConfig {
            min_spacing: Duration::from_secs(1),
            ..open()
        });
        let t = Instant::now();
        c.acquire(Lane::Interactive).await.unwrap();
        c.acquire(Lane::Background).await.unwrap();
        c.acquire(Lane::Delta).await.unwrap();
        assert!(t.elapsed() >= Duration::from_secs(2), "{:?}", t.elapsed());
    }
}
