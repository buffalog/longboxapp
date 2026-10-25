use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client as HttpClient, Response, StatusCode};
use serde::de::DeserializeOwned;
use tracing::{debug, instrument, warn};
use url::Url;

use crate::error::{excerpt, MetronError};
use crate::gate::{Gate, Lane};
use crate::models::{
    MetronIssueDetailRow, MetronIssueListRow, MetronList, MetronSeriesDetailRow,
    MetronSeriesListRow,
};
use crate::projection::{
    project_calendar_item, project_calendar_item_from_detail, project_issue_ref,
    project_series_detail, project_series_ref, MetronCalendarItem, MetronIssueRef,
    MetronSeriesDetail, MetronSeriesRef,
};

const DEFAULT_BASE_URL: &str = "https://metron.cloud/api/";
const DEFAULT_USER_AGENT: &str = "longbox/0.1 (https://github.com/jeremyp/longbox)";
const PAGE_LIMIT: u32 = 500;

/// Configuration for [`MetronClient`].
///
/// Auth: a revocable API token (`METRON_API_TOKEN`) is sent as
/// `Authorization: Bearer <token>`, per Metron's API docs
/// (<https://github.com/Metron-Project/metron/blob/master/api/README.md>,
/// "Token Authentication"). When a token is set, Basic Auth is never
/// sent. Basic Auth with username + password (`METRON_API_USER` /
/// `METRON_API_PASSWORD`) is only the fallback when no token is set.
/// Settings-table storage was considered and rejected — secrets stay out
/// of the DB; restart cycle to rotate credentials is acceptable for a
/// human-managed account.
#[derive(Clone)]
pub struct MetronClientConfig {
    /// Metron API token. When non-empty it wins: `username` and
    /// `password` are ignored.
    pub api_token: String,
    pub username: String,
    pub password: String,
    pub base_url: String,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    /// Minimum gap between request starts, across every caller.
    pub min_interval: Duration,
    /// Requests per UTC day before the client refuses with `RateLimited`.
    pub daily_budget: u32,
    /// Upper bound of the random extra wait added to a 429's `Retry-After`.
    pub jitter_max: Duration,
    pub user_agent: String,
}

/// Redacts the token and password so a `?config` log can't leak them.
impl std::fmt::Debug for MetronClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetronClientConfig")
            .field("api_token", &"REDACTED")
            .field("username", &self.username)
            .field("password", &"REDACTED")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Default for MetronClientConfig {
    fn default() -> Self {
        Self {
            api_token: String::new(),
            username: String::new(),
            password: String::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            timeout: Duration::from_secs(30),
            // Same 5s connect bound as the Bug 5 fix put on the CV client.
            connect_timeout: Duration::from_secs(5),
            // 15/min against Metron's published 20/min burst, and 4,000
            // a day against its 5,000/day sustained limit.
            min_interval: Duration::from_secs(4),
            daily_budget: 4_000,
            jitter_max: Duration::from_secs(5),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
        }
    }
}

#[derive(Clone)]
enum Auth {
    Token(String),
    Basic { username: String, password: String },
}

/// Cheap to clone: every clone, and every [`Self::with_lane`] handle,
/// shares one [`Gate`], so all Metron traffic is coordinated.
#[derive(Clone)]
pub struct MetronClient {
    http: HttpClient,
    base_url: Url,
    auth: Auth,
    gate: Arc<Gate>,
    lane: Lane,
}

impl std::fmt::Debug for MetronClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetronClient")
            .field("base_url", &self.base_url.as_str())
            .field("auth", &self.auth_mode())
            .field("lane", &self.lane)
            .finish_non_exhaustive()
    }
}

impl MetronClient {
    /// Construct a client. Returns [`MetronError::Auth`] if there's
    /// neither a token nor a username + password — surface this at boot
    /// so the feature can be cleanly gated rather than failing per-request.
    pub fn new(config: MetronClientConfig) -> Result<Self, MetronError> {
        let token = config.api_token.trim();
        let auth = if !token.is_empty() {
            Auth::Token(token.to_owned())
        } else if config.username.trim().is_empty() || config.password.trim().is_empty() {
            return Err(MetronError::Auth);
        } else {
            Auth::Basic {
                username: config.username,
                password: config.password,
            }
        };
        let base_url = Url::parse(&config.base_url).map_err(|e| MetronError::Malformed {
            message: format!("invalid base_url: {e}"),
            raw_excerpt: None,
        })?;
        let http = HttpClient::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .user_agent(config.user_agent)
            .build()?;
        let gate = Gate::new(config.min_interval, config.daily_budget, config.jitter_max);
        Ok(Self {
            http,
            base_url,
            auth,
            gate: Arc::new(gate),
            lane: Lane::Interactive,
        })
    }

    /// Which credential this client sends: `"token"` or `"basic"`.
    /// Never the credential itself.
    pub fn auth_mode(&self) -> &'static str {
        match self.auth {
            Auth::Token(_) => "token",
            Auth::Basic { .. } => "basic",
        }
    }

    /// A handle on the same gate that calls Metron from `lane`. A new
    /// client starts on [`Lane::Interactive`].
    pub fn with_lane(&self, lane: Lane) -> Self {
        Self {
            lane,
            ..self.clone()
        }
    }

    /// Fetch issues with `store_date` in `[after, before]`. Paginated like
    /// the CV calendar fetch — chases `next` until exhausted. Returns a
    /// flat `Vec<MetronCalendarItem>` projected from the list-endpoint
    /// shape. **Publisher is `None` on every row in the return value** —
    /// piece 3's hydration step joins it in.
    #[instrument(target = "longbox_metron", skip(self))]
    pub async fn fetch_issues_by_store_date_range(
        &self,
        after: &str,
        before: &str,
    ) -> Result<Vec<MetronCalendarItem>, MetronError> {
        let mut out = Vec::new();
        let mut page: u32 = 1;
        let limit_str = PAGE_LIMIT.to_string();

        loop {
            let page_str = page.to_string();
            let url = self.build_url(
                "issue/",
                &[
                    ("store_date_range_after", after),
                    ("store_date_range_before", before),
                    ("page_size", limit_str.as_str()),
                    ("page", page_str.as_str()),
                ],
            )?;
            let body = self.execute_with_retry(url).await?;
            let envelope: MetronList<MetronIssueListRow> = parse_json(&body)?;
            let total = envelope.count;
            let has_next = envelope.next.is_some();
            for row in envelope.results {
                out.push(project_calendar_item(row));
            }
            if !has_next {
                break;
            }
            if out.len() as i64 >= total {
                break;
            }
            page = page.saturating_add(1);
            debug!(target: "longbox_metron", page, "paginating next issue page");
        }

        Ok(out)
    }

    /// All of a series' issues from Metron (`issue/?series_id=`), paginated.
    /// Returns id + number for each — the issue-linking resolver matches our
    /// issue numbers against these.
    #[instrument(target = "longbox_metron", skip(self))]
    pub async fn fetch_issues_by_series_id(
        &self,
        metron_series_id: i64,
    ) -> Result<Vec<MetronIssueRef>, MetronError> {
        let mut out = Vec::new();
        let mut page: u32 = 1;
        let limit_str = PAGE_LIMIT.to_string();
        let series_str = metron_series_id.to_string();

        loop {
            let page_str = page.to_string();
            let url = self.build_url(
                "issue/",
                &[
                    ("series_id", series_str.as_str()),
                    ("page_size", limit_str.as_str()),
                    ("page", page_str.as_str()),
                ],
            )?;
            let body = self.execute_with_retry(url).await?;
            let envelope: MetronList<MetronIssueListRow> = parse_json(&body)?;
            let total = envelope.count;
            let has_next = envelope.next.is_some();
            for row in envelope.results {
                out.push(project_issue_ref(row));
            }
            if !has_next || out.len() as i64 >= total {
                break;
            }
            page = page.saturating_add(1);
        }

        Ok(out)
    }

    /// Fetch one issue's detail, including publisher inline + the series
    /// id needed for piece 4's cross-reference. Piece 3 uses this either
    /// directly (detail-per-issue hydration) or to learn series ids that
    /// it then fans out to [`Self::fetch_series_detail`].
    #[instrument(target = "longbox_metron", skip(self))]
    pub async fn fetch_issue_detail(
        &self,
        metron_issue_id: i64,
    ) -> Result<MetronCalendarItem, MetronError> {
        let path = format!("issue/{metron_issue_id}/");
        let url = self.build_url(&path, &[])?;
        let body = self.execute_with_retry(url).await?;
        let detail: MetronIssueDetailRow = parse_json(&body)?;
        Ok(project_calendar_item_from_detail(detail))
    }

    /// Fetch one series's detail (publisher, year range, CV cross-
    /// reference). The per-unique-series hydration path in piece 3
    /// composes this on top of [`Self::fetch_issues_by_store_date_range`].
    #[instrument(target = "longbox_metron", skip(self))]
    pub async fn fetch_series_detail(
        &self,
        metron_series_id: i64,
    ) -> Result<MetronSeriesDetail, MetronError> {
        let path = format!("series/{metron_series_id}/");
        let url = self.build_url(&path, &[])?;
        let body = self.execute_with_retry(url).await?;
        let detail: MetronSeriesDetailRow = parse_json(&body)?;
        Ok(project_series_detail(detail))
    }

    /// Resolve a LongBox CV-keyed series to its Metron counterpart via
    /// the `?cv_id=` reverse-mapping. Returns `None` when Metron has no
    /// record cross-referenced to that CV id. Piece 4 calls this in bulk
    /// to populate `series.metron_id` on the catalog.
    #[instrument(target = "longbox_metron", skip(self))]
    pub async fn fetch_series_by_cv_id(
        &self,
        cv_id: i64,
    ) -> Result<Option<MetronSeriesRef>, MetronError> {
        let cv_id_str = cv_id.to_string();
        let url = self.build_url("series/", &[("cv_id", cv_id_str.as_str())])?;
        let body = self.execute_with_retry(url).await?;
        let envelope: MetronList<MetronSeriesListRow> = parse_json(&body)?;
        Ok(envelope.results.into_iter().next().map(project_series_ref))
    }

    // -------- internals --------

    fn build_url(&self, path: &str, params: &[(&str, &str)]) -> Result<Url, MetronError> {
        let mut url = self
            .base_url
            .join(path)
            .map_err(|e| MetronError::Malformed {
                message: format!("could not build URL: {e}"),
                raw_excerpt: None,
            })?;
        if !params.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in params {
                pairs.append_pair(k, v);
            }
        }
        Ok(url)
    }

    /// Send through the gate, holding the slot until the body is read.
    /// A 429 pauses every caller (see [`Gate::note_429`]); this request
    /// then retries once, through the gate like any other.
    async fn execute_with_retry(&self, url: Url) -> Result<String, MetronError> {
        let mut retried = false;
        loop {
            let _slot = self.gate.acquire(self.lane).await?;
            let resp = self.send(&url).await?;
            self.gate.note_headers(resp.headers());
            if resp.status() != StatusCode::TOO_MANY_REQUESTS {
                return handle_response(resp).await;
            }
            let pause = self.gate.note_429(parse_retry_after(&resp));
            warn!(
                target: "longbox_metron",
                lane = ?self.lane,
                pause_secs = pause.as_secs(),
                retried,
                "received 429; pausing all Metron callers"
            );
            if retried {
                return Err(MetronError::RateLimited {
                    retry_after_seconds: pause.as_secs(),
                });
            }
            retried = true;
        }
    }

    async fn send(&self, url: &Url) -> Result<Response, MetronError> {
        if tracing::enabled!(target: "longbox_metron", tracing::Level::DEBUG) {
            debug!(target: "longbox_metron", url = %url, "sending Metron request");
        }
        let req = self.http.get(url.clone());
        let req = match &self.auth {
            Auth::Token(token) => req.bearer_auth(token),
            Auth::Basic { username, password } => req.basic_auth(username, Some(password)),
        };
        Ok(req.header("Accept", "application/json").send().await?)
    }
}

async fn handle_response(resp: Response) -> Result<String, MetronError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp.text().await?);
    }
    match status.as_u16() {
        401 => Err(MetronError::Auth),
        404 => Err(MetronError::NotFound),
        s => {
            let body = resp.text().await.unwrap_or_default();
            Err(MetronError::Http { status: s, body })
        }
    }
}

fn parse_json<T: DeserializeOwned>(body: &str) -> Result<T, MetronError> {
    serde_json::from_str::<T>(body).map_err(|e| MetronError::Malformed {
        message: format!("JSON parse error: {e}"),
        raw_excerpt: excerpt(body),
    })
}

/// Parse `Retry-After`. Accept integer seconds; default to 60s if missing
/// or malformed.
fn parse_retry_after(resp: &Response) -> Duration {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_username_is_rejected() {
        let cfg = MetronClientConfig {
            password: "x".into(),
            ..Default::default()
        };
        let err = MetronClient::new(cfg).unwrap_err();
        assert!(matches!(err, MetronError::Auth));
    }

    #[test]
    fn empty_password_is_rejected() {
        let cfg = MetronClientConfig {
            username: "x".into(),
            ..Default::default()
        };
        let err = MetronClient::new(cfg).unwrap_err();
        assert!(matches!(err, MetronError::Auth));
    }

    #[test]
    fn whitespace_only_credentials_are_rejected() {
        let cfg = MetronClientConfig {
            username: "  ".into(),
            password: "  ".into(),
            ..Default::default()
        };
        let err = MetronClient::new(cfg).unwrap_err();
        assert!(matches!(err, MetronError::Auth));
    }
}
