use thiserror::Error;

/// Errors surfaced from [`crate::ComicVineClient`].
///
/// Web-layer handlers map these to HTTP responses:
/// - `RateLimited` → 503 with `Retry-After`
/// - `Network` / `Timeout` → 504
/// - `Malformed` → 502
/// - `Auth` → 500 (config issue surfaced to logs, not the user)
/// - `NotFound` → 404 (`fetch_volume` + `fetch_issue_credits`)
/// - `Http { status: 5xx }` → 502 with the status echoed
#[derive(Debug, Error)]
pub enum CvError {
    #[error("network error: {0}")]
    Network(reqwest::Error),

    #[error("authentication failed (HTTP 401 or CV status_code 100)")]
    Auth,

    #[error("rate limited; retry after {retry_after_seconds}s")]
    RateLimited { retry_after_seconds: u64 },

    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },

    #[error("malformed CV response: {message}")]
    Malformed {
        message: String,
        raw_excerpt: Option<String>,
    },

    #[error("request timed out")]
    Timeout,

    #[error("CV volume not found")]
    NotFound,
}

impl From<reqwest::Error> for CvError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            CvError::Timeout
        } else {
            CvError::Network(err)
        }
    }
}

/// Helper: truncate a response body for inclusion in `Malformed.raw_excerpt`.
/// We never want to attach 5MB of HTML to an error.
pub(crate) fn excerpt(body: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    const MAX: usize = 500;
    if body.len() <= MAX {
        Some(body.to_owned())
    } else {
        // Find a char boundary at or before MAX.
        let mut end = MAX;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        Some(format!("{}…", &body[..end]))
    }
}
