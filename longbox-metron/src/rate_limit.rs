//! Token-bucket rate limiter wrapping `governor`. One bucket per client
//! instance; all Metron methods share it.
//!
//! Metron's published limits (from response headers observed during the
//! Item A v2 archaeology pass):
//!   - burst:     20 requests
//!   - sustained: 5000 (window unspecified — likely 5000/day)
//!
//! We model this conservatively as a single bucket: refill rate 1 token /
//! second (≈ 3600/hour, comfortably below the 5000/day ceiling regardless
//! of window interpretation), burst capacity 20 matching the published
//! burst. The forward-calendar use case is well inside this envelope —
//! a typical "Next week" fetch + hydration round will use a few dozen
//! tokens out of 20-burst + drip refill, and the 24h cv_volume_cache TTL
//! ensures we don't refetch the same week multiple times per day.

use std::num::NonZeroU32;
use std::time::Duration;

use governor::clock::{Clock, DefaultClock};
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

use crate::error::MetronError;

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

pub struct MetronRateLimiter {
    limiter: Limiter,
    max_wait_for_slot: Duration,
}

impl MetronRateLimiter {
    /// Build a limiter with the given rate (tokens/hour) and burst capacity.
    /// `max_wait_for_slot` is how long `acquire` will block waiting for a
    /// token before giving up with [`MetronError::RateLimited`].
    pub fn new(rate_per_hour: u32, burst: u32, max_wait_for_slot: Duration) -> Self {
        let rate = rate_per_hour.max(1);
        let burst_nz = NonZeroU32::new(burst.max(1)).expect("burst must be non-zero");
        let period_nanos = 3_600_000_000_000_u64 / u64::from(rate);
        let period = Duration::from_nanos(period_nanos.max(1));
        let quota = Quota::with_period(period)
            .expect("rate must yield a non-zero period")
            .allow_burst(burst_nz);
        Self {
            limiter: RateLimiter::direct(quota),
            max_wait_for_slot,
        }
    }

    /// Block until a token is available (up to `max_wait_for_slot`), then
    /// consume it. Returns [`MetronError::RateLimited`] if no token frees up
    /// in time; `retry_after_seconds` is governor's reported wait,
    /// rounded up.
    pub async fn acquire(&self) -> Result<(), MetronError> {
        let clock = DefaultClock::default();
        let deadline = tokio::time::Instant::now() + self.max_wait_for_slot;

        loop {
            match self.limiter.check() {
                Ok(()) => return Ok(()),
                Err(not_until) => {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        let wait = not_until.wait_time_from(clock.now());
                        return Err(MetronError::RateLimited {
                            retry_after_seconds: wait.as_secs().saturating_add(1),
                        });
                    }
                    let wait = not_until.wait_time_from(clock.now());
                    let to_sleep = wait.min(deadline.duration_since(now));
                    tokio::time::sleep(to_sleep).await;
                }
            }
        }
    }
}
