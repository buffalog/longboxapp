use thiserror::Error;

/// Errors surfaced from [`crate::MetronClient`].
///
/// Web-layer handlers map these to HTTP responses:
/// - `RateLimited` → 503 with `Retry-After`
/// - `Network` / `Timeout` → 504
/// - `Malformed` → 502
/// - `Auth` → 500 (config issue surfaced to logs, not the user — keys / creds
///   wrong in the deploy env, not something the user can fix from the browser)
/// - `NotFound` → 404 (`fetch_issue_detail` / `fetch_series_detail` produce
///   this when Metron returns 404)
/// - `Http { status: 5xx }` → 502 with the status echoed
#[derive(Debug, Error)]
pub enum MetronError {
    #[error("network error: {0}")]
    Network(reqwest::Error),

    #[error("authentication failed (HTTP 401 or missing credentials)")]
    Auth,

    #[error("rate limited; retry after {retry_after_seconds}s")]
    RateLimited { retry_after_seconds: u64 },

    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },

    #[error("malformed Metron response: {message}")]
    Malformed {
        message: String,
        raw_excerpt: Option<String>,
    },

    #[error("request timed out")]
    Timeout,

    #[error("Metron resource not found")]
    NotFound,
}

impl From<reqwest::Error> for MetronError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            MetronError::Timeout
        } else {
            MetronError::Network(err)
        }
    }
}

/// Helper: truncate a response body for inclusion in `Malformed.raw_excerpt`.
/// Mirrors `longbox-comicvine`'s helper — never want 5MB of HTML attached to
/// an error.
pub(crate) fn excerpt(body: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    const MAX: usize = 500;
    if body.len() <= MAX {
        Some(body.to_owned())
    } else {
        let mut end = MAX;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        Some(body[..end].to_owned())
    }
}
