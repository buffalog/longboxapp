//! Token-bucket rate limiter wrapping `governor`. One bucket per client
//! instance; all CV methods share it.

use std::num::NonZeroU32;
use std::time::Duration;

use governor::clock::{Clock, DefaultClock};
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

use crate::error::CvError;

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

pub struct CvRateLimiter {
    limiter: Limiter,
    max_wait_for_slot: Duration,
}

impl CvRateLimiter {
    /// Build a limiter with the given rate (cells/hour) and burst capacity.
    /// `max_wait_for_slot` is how long `acquire` will block waiting for a
    /// token before giving up with [`CvError::RateLimited`].
    pub fn new(rate_per_hour: u32, burst: u32, max_wait_for_slot: Duration) -> Self {
        let rate = rate_per_hour.max(1);
        let burst_nz = NonZeroU32::new(burst.max(1)).expect("burst must be non-zero");
        // Use nanosecond precision so high rates (>3600/hr) still yield a
        // non-zero period. Integer-second division would truncate to 0.
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
    /// consume it. Returns [`CvError::RateLimited`] if no token frees up in
    /// time; `retry_after_seconds` is governor's reported wait, rounded up.
    pub async fn acquire(&self) -> Result<(), CvError> {
        let clock = DefaultClock::default();
        let deadline = tokio::time::Instant::now() + self.max_wait_for_slot;

        loop {
            match self.limiter.check() {
                Ok(()) => return Ok(()),
                Err(not_until) => {
                    let wait = not_until.wait_time_from(clock.now());
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if wait > remaining {
                        let secs = wait
                            .as_secs()
                            .saturating_add(u64::from(wait.subsec_nanos() > 0));
                        return Err(CvError::RateLimited {
                            retry_after_seconds: secs,
                        });
                    }
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// 3600/hour = one cell per second. Burst 3 = three quick calls allowed.
    fn fast_limiter(burst: u32, max_wait: Duration) -> CvRateLimiter {
        CvRateLimiter::new(3600, burst, max_wait)
    }

    #[tokio::test]
    async fn burst_capacity_allows_quick_acquires() {
        let limiter = fast_limiter(3, Duration::from_secs(5));
        let start = Instant::now();
        for _ in 0..3 {
            limiter.acquire().await.unwrap();
        }
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "3 acquires within burst should be near-instant, took {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn blocks_then_succeeds_when_slot_frees_up() {
        // Burst 1, one cell/sec. The second acquire must wait ~1s.
        let limiter = fast_limiter(1, Duration::from_secs(3));
        limiter.acquire().await.unwrap();

        let start = Instant::now();
        limiter.acquire().await.unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(800),
            "expected ~1s wait, got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(2000),
            "wait shouldn't exceed slot period+slack, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn returns_rate_limited_when_slot_does_not_free_in_time() {
        let limiter = fast_limiter(1, Duration::from_millis(100));
        limiter.acquire().await.unwrap();

        let start = Instant::now();
        let err = limiter.acquire().await.unwrap_err();
        let elapsed = start.elapsed();
        match err {
            CvError::RateLimited {
                retry_after_seconds,
            } => {
                // Wait until next slot is ~1s (one cell/sec).
                assert!(
                    retry_after_seconds >= 1,
                    "retry_after_seconds should be >= 1, got {retry_after_seconds}"
                );
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
        assert!(
            elapsed < Duration::from_millis(500),
            "should bail quickly when wait exceeds max_wait_for_slot, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn successive_acquires_consume_tokens() {
        let limiter = fast_limiter(2, Duration::from_millis(100));
        limiter.acquire().await.unwrap();
        limiter.acquire().await.unwrap();

        // Third one in the same instant should rate-limit; bucket only holds 2.
        let err = limiter.acquire().await.unwrap_err();
        assert!(matches!(err, CvError::RateLimited { .. }), "got {err:?}");
    }
}
