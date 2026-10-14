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
//! - the per-lane call counters behind the rate-limit chip.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::error::CvError;
use crate::rate_limit::CvRateLimiter;

/// Which budget a request spends, and how it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// User-initiated routes. Interactive bucket; a request that can't get
    /// a token within `interactive_max_wait` fails fast with
    /// [`CvError::RateLimited`] instead of hanging the page.
    Interactive,
    /// Enrichment, the credits resolver, the full catalog refresh and the
    /// calendar's bulk-subscribe resolves. Background bucket; waits as long
    /// as it takes.
    Background,
    /// The delta refresh at the top of each pull sweep. Spends the
    /// *interactive* bucket, because its ~10 calendar pages would take
    /// ~7 minutes at the background rate, but waits like background
    /// (never fails a sweep with `RateLimited`).
    Delta,
}

/// Lane budgets. Read from settings at boot; a change needs a restart.
#[derive(Debug, Clone)]
pub struct CvLanesConfig {
    pub interactive_rate_per_hour: u32,
    pub interactive_burst: u32,
    pub interactive_max_wait: Duration,
    pub background_rate_per_hour: u32,
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
            interactive_max_wait: Duration::from_secs(10),
            background_rate_per_hour: 80,
            min_spacing: Duration::from_secs(1),
        }
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
}

pub struct CvCoordinator {
    config: CvLanesConfig,
    interactive: CvRateLimiter,
    background: CvRateLimiter,
    interactive_calls: CallCounter,
    background_calls: CallCounter,
    /// Earliest instant the next request may go out (the pacer).
    next_slot: Mutex<Option<Instant>>,
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
            next_slot: Mutex::new(None),
            config,
        }
    }

    /// Wait until `lane` may send one request, then count it.
    pub async fn acquire(&self, lane: Lane) -> Result<(), CvError> {
        match lane {
            Lane::Interactive => {
                self.interactive
                    .acquire(self.config.interactive_max_wait)
                    .await?
            }
            Lane::Delta => self.interactive.acquire(PATIENT_WAIT).await?,
            Lane::Background => self.background.acquire(PATIENT_WAIT).await?,
        }
        self.pace().await;
        match lane {
            Lane::Interactive | Lane::Delta => self.interactive_calls.consume(),
            Lane::Background => self.background_calls.consume(),
        }
        Ok(())
    }

    /// Reserve the next send slot, `min_spacing` after the previous one,
    /// and sleep until it.
    async fn pace(&self) {
        let slot = {
            let mut next = self.next_slot.lock().expect("pacer mutex poisoned");
            let now = Instant::now();
            let slot = next.map_or(now, |n| n.max(now));
            *next = Some(slot + self.config.min_spacing);
            slot
        };
        tokio::time::sleep_until(slot).await;
    }

    pub fn snapshot(&self) -> CvLanesSnapshot {
        CvLanesSnapshot {
            interactive: self
                .interactive_calls
                .snapshot(self.config.interactive_rate_per_hour),
            background: self
                .background_calls
                .snapshot(self.config.background_rate_per_hour),
        }
    }
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
