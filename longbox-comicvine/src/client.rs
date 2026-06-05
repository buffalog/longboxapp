use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::Duration;

use reqwest::{Client as HttpClient, Response, StatusCode};
use serde::de::DeserializeOwned;
use tracing::{debug, instrument, warn};
use url::Url;

use crate::error::{excerpt, CvError};
use crate::models::{CvIssueFull, CvResponse, CvVolumeFull, CvVolumeSearchItem};
use crate::projection::{
    project_calendar_item, project_issue, project_search_item, project_volume, CvCalendarItem,
    CvIssueDetail, CvVolumeDetail, SeriesSearchResult,
};
use crate::rate_limit::CvRateLimiter;

const DEFAULT_BASE_URL: &str = "https://comicvine.gamespot.com/api/";
const DEFAULT_USER_AGENT: &str = "longbox/0.1 (https://github.com/jeremyp/longbox)";
const PAGE_LIMIT: u32 = 100;
const BURST_CAPACITY: u32 = 5;

#[derive(Debug, Clone)]
pub struct ComicVineClientConfig {
    pub api_key: String,
    pub base_url: String,
    pub timeout: Duration,
    /// Caps the connect/DNS/TLS-handshake phase independently of
    /// `timeout` (which bounds the whole request). Without this,
    /// flaky connects can stall for minutes through OS-level TCP
    /// retries and glibc resolver chains, dragging the enrichment
    /// worker's pace.
    pub connect_timeout: Duration,
    pub rate_limit_per_hour: u32,
    pub max_wait_for_slot: Duration,
    pub user_agent: String,
}

impl Default for ComicVineClientConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            rate_limit_per_hour: 180,
            max_wait_for_slot: Duration::from_secs(60),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
        }
    }
}

/// Snapshot of the per-hour call counter — what the `/api/cv/rate-limit`
/// endpoint reflects up to the frontend. `count` is the number of
/// successful `acquire()` calls inside the current sliding window;
/// `limit_per_hour` is the configured cap; `window_started_at` is the
/// unix-second when the current window opened (first call after the
/// previous window expired, or process boot for the very first call).
#[derive(Debug, Clone, Copy)]
pub struct CvRateLimitSnapshot {
    pub count: u32,
    pub limit_per_hour: u32,
    pub window_started_at_unix: i64,
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
        let (count, window_started_at_unix) =
            if start == 0 || now - start >= Self::WINDOW_SECS {
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

pub struct ComicVineClient {
    http: HttpClient,
    base_url: Url,
    api_key: String,
    limiter: CvRateLimiter,
    calls: CallCounter,
    rate_limit_per_hour: u32,
}

impl std::fmt::Debug for ComicVineClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComicVineClient")
            .field("base_url", &self.base_url.as_str())
            .field("api_key", &"REDACTED")
            .finish_non_exhaustive()
    }
}

impl ComicVineClient {
    /// Construct a client. Returns an error if `api_key` is empty or
    /// `base_url` is not a valid URL.
    pub fn new(config: ComicVineClientConfig) -> Result<Self, CvError> {
        if config.api_key.trim().is_empty() {
            return Err(CvError::Auth);
        }
        let base_url = Url::parse(&config.base_url).map_err(|e| CvError::Malformed {
            message: format!("invalid base_url: {e}"),
            raw_excerpt: None,
        })?;
        let http = HttpClient::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .user_agent(config.user_agent)
            .build()?;
        let limiter = CvRateLimiter::new(
            config.rate_limit_per_hour,
            BURST_CAPACITY,
            config.max_wait_for_slot,
        );
        Ok(Self {
            http,
            base_url,
            api_key: config.api_key,
            limiter,
            calls: CallCounter::new(),
            rate_limit_per_hour: config.rate_limit_per_hour,
        })
    }

    /// Read-only view of the rolling per-hour call count. Drives the
    /// frontend's CV rate-limit chip.
    pub fn rate_limit_snapshot(&self) -> CvRateLimitSnapshot {
        self.calls.snapshot(self.rate_limit_per_hour)
    }

    #[instrument(target = "longbox_comicvine", skip(self), fields(query = %query))]
    pub async fn search_volumes(&self, query: &str) -> Result<Vec<SeriesSearchResult>, CvError> {
        let limit = PAGE_LIMIT.to_string();
        let url = self.build_url(
            "search/",
            &[
                ("resources", "volume"),
                ("query", query),
                ("limit", limit.as_str()),
            ],
        )?;
        let body = self.execute_with_retry(url).await?;
        let envelope = parse_envelope::<Vec<CvVolumeSearchItem>>(&body)?;
        let results = unwrap_envelope_results(envelope, &body)?;
        Ok(results.into_iter().map(project_search_item).collect())
    }

    #[instrument(target = "longbox_comicvine", skip(self))]
    pub async fn fetch_volume(&self, cv_volume_id: i64) -> Result<CvVolumeDetail, CvError> {
        let path = format!("volume/4050-{cv_volume_id}/");
        let url = self.build_url(&path, &[])?;
        let body = self.execute_with_retry(url).await?;
        let envelope = parse_envelope::<CvVolumeFull>(&body)?;
        let volume = unwrap_envelope_results(envelope, &body)?;
        Ok(project_volume(volume))
    }

    #[instrument(target = "longbox_comicvine", skip(self))]
    pub async fn fetch_issues(&self, cv_volume_id: i64) -> Result<Vec<CvIssueDetail>, CvError> {
        let mut out = Vec::new();
        let mut offset: u32 = 0;
        let filter = format!("volume:{cv_volume_id}");
        let limit_str = PAGE_LIMIT.to_string();

        loop {
            let offset_str = offset.to_string();
            let url = self.build_url(
                "issues/",
                &[
                    ("filter", filter.as_str()),
                    ("limit", limit_str.as_str()),
                    ("offset", offset_str.as_str()),
                ],
            )?;
            let body = self.execute_with_retry(url).await?;
            let envelope = parse_envelope::<Vec<CvIssueFull>>(&body)?;
            let total = envelope.number_of_total_results;
            let page = unwrap_envelope_results(envelope, &body)?;
            let returned = page.len() as u32;
            for issue in page {
                out.push(project_issue(issue));
            }

            // Stop when we've collected everything, or when CV returns a
            // short page (defensive against infinite loops if totals lie).
            if returned == 0 {
                break;
            }
            if total <= offset.saturating_add(returned) {
                break;
            }
            offset = offset.saturating_add(returned);
            debug!(target: "longbox_comicvine", offset, "paginating next issue page");
        }

        Ok(out)
    }

    /// Fetch every issue with a `store_date` inside `[from, to]` — the
    /// release-calendar query, paginated like [`Self::fetch_issues`].
    /// `from` / `to` are `YYYY-MM-DD`. Issues CV returns without a
    /// `store_date` or `volume` are dropped by the projection.
    #[instrument(target = "longbox_comicvine", skip(self))]
    pub async fn fetch_release_calendar(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<CvCalendarItem>, CvError> {
        let mut out = Vec::new();
        let mut offset: u32 = 0;
        let filter = format!("store_date:{from}|{to}");
        let limit_str = PAGE_LIMIT.to_string();

        loop {
            let offset_str = offset.to_string();
            let url = self.build_url(
                "issues/",
                &[
                    ("filter", filter.as_str()),
                    ("limit", limit_str.as_str()),
                    ("offset", offset_str.as_str()),
                ],
            )?;
            let body = self.execute_with_retry(url).await?;
            let envelope = parse_envelope::<Vec<CvIssueFull>>(&body)?;
            let total = envelope.number_of_total_results;
            let page = unwrap_envelope_results(envelope, &body)?;
            let returned = page.len() as u32;
            for issue in page {
                if let Some(item) = project_calendar_item(issue) {
                    out.push(item);
                }
            }

            if returned == 0 {
                break;
            }
            if total <= offset.saturating_add(returned) {
                break;
            }
            offset = offset.saturating_add(returned);
            debug!(target: "longbox_comicvine", offset, "paginating next calendar page");
        }

        Ok(out)
    }

    fn build_url(&self, path: &str, params: &[(&str, &str)]) -> Result<Url, CvError> {
        let mut url = self.base_url.join(path).map_err(|e| CvError::Malformed {
            message: format!("could not build URL: {e}"),
            raw_excerpt: None,
        })?;
        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in params {
                pairs.append_pair(k, v);
            }
            pairs.append_pair("api_key", &self.api_key);
            pairs.append_pair("format", "json");
        }
        Ok(url)
    }

    async fn execute_with_retry(&self, url: Url) -> Result<String, CvError> {
        self.limiter.acquire().await?;
        self.calls.consume();
        let resp = self.send(&url).await?;

        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = parse_retry_after(&resp);
            warn!(
                target: "longbox_comicvine",
                retry_after_secs = retry_after.as_secs(),
                "received 429; sleeping and retrying once"
            );
            tokio::time::sleep(retry_after).await;

            self.limiter.acquire().await?;
            self.calls.consume();
            let resp2 = self.send(&url).await?;
            if resp2.status() == StatusCode::TOO_MANY_REQUESTS {
                let secs = parse_retry_after(&resp2).as_secs();
                return Err(CvError::RateLimited {
                    retry_after_seconds: secs,
                });
            }
            return handle_response(resp2).await;
        }

        handle_response(resp).await
    }

    async fn send(&self, url: &Url) -> Result<Response, CvError> {
        // Log the URL with the api_key value redacted so spans / dev logs
        // don't leak it. We rebuild the redacted form just for the log line.
        if tracing::enabled!(target: "longbox_comicvine", tracing::Level::DEBUG) {
            let redacted = redact_api_key(url);
            debug!(target: "longbox_comicvine", url = %redacted, "sending CV request");
        }
        Ok(self.http.get(url.clone()).send().await?)
    }
}

async fn handle_response(resp: Response) -> Result<String, CvError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp.text().await?);
    }
    match status.as_u16() {
        401 => Err(CvError::Auth),
        404 => Err(CvError::NotFound),
        s => {
            let body = resp.text().await.unwrap_or_default();
            Err(CvError::Http { status: s, body })
        }
    }
}

fn parse_envelope<T: DeserializeOwned>(body: &str) -> Result<CvResponse<T>, CvError> {
    serde_json::from_str::<CvResponse<T>>(body).map_err(|e| CvError::Malformed {
        message: format!("JSON parse error: {e}"),
        raw_excerpt: excerpt(body),
    })
}

/// Validate the envelope `status_code` and unwrap the `results` field. CV
/// returns `results: null` on errors (e.g. status_code 101), so we check the
/// status first and only require `Some(results)` for status_code = 1.
fn unwrap_envelope_results<T>(env: CvResponse<T>, body: &str) -> Result<T, CvError> {
    match env.status_code {
        1 => env.results.ok_or_else(|| CvError::Malformed {
            message: "CV returned status_code 1 but missing `results`".to_owned(),
            raw_excerpt: excerpt(body),
        }),
        100 => Err(CvError::Auth),
        101 => Err(CvError::NotFound),
        other => Err(CvError::Malformed {
            message: format!("CV status_code {other}: {}", env.error),
            raw_excerpt: excerpt(body),
        }),
    }
}

fn parse_retry_after(resp: &Response) -> Duration {
    resp.headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(60))
}

fn redact_api_key(url: &Url) -> Url {
    let mut redacted = url.clone();
    let pairs: Vec<(String, String)> = redacted
        .query_pairs()
        .map(|(k, v)| {
            let key = k.into_owned();
            let value = if key == "api_key" {
                "REDACTED".to_owned()
            } else {
                v.into_owned()
            };
            (key, value)
        })
        .collect();
    redacted.query_pairs_mut().clear();
    {
        let mut builder = redacted.query_pairs_mut();
        for (k, v) in &pairs {
            builder.append_pair(k, v);
        }
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_api_key_replaces_value() {
        let url = Url::parse(
            "https://example.com/api/search/?resources=volume&query=saga&api_key=SECRET&format=json",
        )
        .unwrap();
        let redacted = redact_api_key(&url);
        let query = redacted.query().unwrap();
        assert!(query.contains("api_key=REDACTED"));
        assert!(!query.contains("SECRET"));
        // Other params still present.
        assert!(query.contains("resources=volume"));
        assert!(query.contains("query=saga"));
    }

    #[test]
    fn empty_api_key_is_rejected() {
        let cfg = ComicVineClientConfig {
            api_key: "  ".into(),
            ..Default::default()
        };
        let err = ComicVineClient::new(cfg).unwrap_err();
        assert!(matches!(err, CvError::Auth));
    }
}
