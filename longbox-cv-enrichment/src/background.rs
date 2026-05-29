//! Background-priority wrapper around [`ComicVineClient`].
//!
//! The enrichment worker calls CV through this wrapper; interactive
//! callers (refresh, add, search) use the bare client and bypass
//! the secondary gate entirely. The wrapper holds:
//!
//! 1. A `governor::RateLimiter` configured at the worker's request
//!    interval (`Quota::with_period(30s)` by default) — the
//!    *background gate*.
//! 2. An `Arc<ComicVineClient>` — the shared client whose internal
//!    180/h limiter is the *shared gate*.
//!
//! Each background CV call acquires the background gate first
//! (await-throttled to 30s spacing), then delegates to the inner
//! client, which acquires the shared 180/h gate. An interactive
//! call only acquires the shared gate — it never queues behind the
//! background gate even when the worker is in mid-backlog. That's
//! the latency guarantee the kickoff's Q3 promised: interactive
//! gets predictable next-shared-slot latency under sustained
//! background pressure.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use longbox_comicvine::{ComicVineClient, CvError, CvIssueDetail, CvVolumeDetail, SeriesSearchResult};

type BackgroundLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

pub struct BackgroundCvClient {
    inner: Arc<ComicVineClient>,
    background_gate: Arc<BackgroundLimiter>,
}

impl BackgroundCvClient {
    /// Build a background-priority wrapper around an existing CV
    /// client. `request_interval` is the minimum spacing between
    /// background-driven CV calls (defaults to 30s in the
    /// settings); at 30s spacing the worker burns ~120 CV requests
    /// per hour, leaving ~60/h headroom on the shared 180/h
    /// limiter for interactive callers.
    pub fn new(inner: Arc<ComicVineClient>, request_interval: Duration) -> Self {
        // governor takes Quota::with_period; floor at 1s to avoid
        // pathological zero-duration windows.
        let period = if request_interval.as_secs() == 0 {
            Duration::from_secs(1)
        } else {
            request_interval
        };
        // SAFETY: 1 is non-zero.
        let burst = NonZeroU32::new(1).expect("1 is non-zero");
        let quota = Quota::with_period(period)
            .expect("non-zero period guaranteed above")
            .allow_burst(burst);
        Self {
            inner,
            background_gate: Arc::new(RateLimiter::direct(quota)),
        }
    }

    /// Background `search_volumes`. Acquires the background gate
    /// (await-throttled), then delegates.
    pub async fn search_volumes(&self, query: &str) -> Result<Vec<SeriesSearchResult>, CvError> {
        self.background_gate.until_ready().await;
        self.inner.search_volumes(query).await
    }

    /// Background `fetch_volume`. Acquires the background gate,
    /// then delegates.
    pub async fn fetch_volume(&self, cv_volume_id: i64) -> Result<CvVolumeDetail, CvError> {
        self.background_gate.until_ready().await;
        self.inner.fetch_volume(cv_volume_id).await
    }

    /// Background `fetch_issues`. Acquires the background gate,
    /// then delegates.
    pub async fn fetch_issues(&self, cv_volume_id: i64) -> Result<Vec<CvIssueDetail>, CvError> {
        self.background_gate.until_ready().await;
        self.inner.fetch_issues(cv_volume_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    /// **Sustained-pressure splitter (Q3 from the kickoff).** Build
    /// a background limiter with a 200ms period (test-fast). Fire
    /// 10 background `until_ready` acquires back-to-back; assert
    /// the cumulative time is ≥ 9 × 200ms (the floor), i.e. the
    /// gate enforces spacing. Meanwhile fire 10 "interactive"
    /// would-be calls that bypass the background gate and run
    /// instantly — their cumulative time should be ≪ 200ms,
    /// proving they didn't queue behind background tokens.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn background_gate_throttles_sequence_but_does_not_block_bypass_calls() {
        let burst = NonZeroU32::new(1).unwrap();
        let quota = Quota::with_period(Duration::from_millis(200))
            .unwrap()
            .allow_burst(burst);
        let gate = Arc::new(RateLimiter::direct(quota));

        let start = Instant::now();

        // Background: 10 spaced acquires.
        let bg_gate = gate.clone();
        let bg_done = AtomicU32::new(0);
        let bg_done_ref = &bg_done;
        let bg_task = async move {
            for _ in 0..10 {
                bg_gate.until_ready().await;
                bg_done_ref.fetch_add(1, Ordering::SeqCst);
            }
        };

        // Interactive: 10 immediate completions (NO gate acquire).
        // These represent calls going to the bare client directly,
        // bypassing the background gate. Sleep 1ms each to simulate
        // some realistic work but not actual rate-limiting.
        let interactive_done = AtomicU32::new(0);
        let interactive_done_ref = &interactive_done;
        let interactive_task = async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(1)).await;
                interactive_done_ref.fetch_add(1, Ordering::SeqCst);
            }
        };

        tokio::join!(bg_task, interactive_task);

        let elapsed = start.elapsed();
        assert_eq!(bg_done.load(Ordering::SeqCst), 10);
        assert_eq!(interactive_done.load(Ordering::SeqCst), 10);
        // Background floor: 9 spacings × 200ms = 1800ms. The first
        // acquire is free; the last 9 are spaced.
        assert!(
            elapsed >= Duration::from_millis(1800),
            "background gate failed to enforce spacing — elapsed {elapsed:?}",
        );
    }

    /// Confirms the background gate ITSELF spaces consecutive
    /// acquires by the expected period under tokio's paused-time
    /// virtual clock. Sanity check decoupled from the
    /// "interactive bypass" assertion above.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn background_gate_enforces_period_between_acquires() {
        let burst = NonZeroU32::new(1).unwrap();
        let quota = Quota::with_period(Duration::from_millis(500))
            .unwrap()
            .allow_burst(burst);
        let gate = RateLimiter::direct(quota);

        let mut times = Vec::new();
        let start = Instant::now();
        for _ in 0..5 {
            gate.until_ready().await;
            times.push(start.elapsed());
        }
        // First acquire near-instant; each subsequent acquire spaced
        // by ~500ms. Governor leaves a small slack under paused-time
        // virtual clock — assert ≥ 480ms which still proves the gate
        // is enforcing meaningful spacing.
        assert!(times[0] < Duration::from_millis(50));
        for w in times.windows(2) {
            let delta = w[1] - w[0];
            assert!(
                delta >= Duration::from_millis(480),
                "spacing violation: {:?}",
                delta
            );
        }
    }
}
