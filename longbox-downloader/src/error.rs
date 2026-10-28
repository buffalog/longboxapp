use thiserror::Error;

/// A downloader operation failure. Every message is redacted when
/// displayed (`longbox_core::redact::secrets`): SAB's request URL carries
/// its apikey and the indexer's NZB link, and SAB's own error text can
/// echo either. Call sites also use reqwest's `without_url()`.
/// As in `longbox-newznab`, the variant
/// carries the permanent-vs-transient signal so Step 6's retry table
/// (submission failure → exponential backoff) matches on it directly
/// rather than introspecting strings.
#[derive(Error)]
pub enum DownloaderError {
    /// Network failure, timeout, or non-success HTTP status (other
    /// than auth). Transient — worth a backoff retry.
    #[error("http request failed: {}", longbox_core::redact::secrets(.0))]
    HttpFailure(String),

    /// The downloader rejected our credentials — bad SABnzbd apikey,
    /// bad NZBGet Basic-auth. Permanent: retrying won't help, the user
    /// must fix the config.
    #[error("downloader rejected credentials: {}", longbox_core::redact::secrets(.0))]
    AuthFailed(String),

    /// The downloader accepted the request but reported an error
    /// (SAB `{"status": false, "error": ...}`, NZBGet JSON-RPC
    /// `error` envelope, an `append` returning a non-positive id).
    #[error("downloader reported an error: {}", longbox_core::redact::secrets(.0))]
    ApiError(String),

    /// Response body didn't parse as the expected shape.
    #[error("malformed downloader response: {}", longbox_core::redact::secrets(.0))]
    MalformedResponse(String),
}

/// Redacted like Display: a derived Debug would print the raw message
/// wherever an error is logged with `?`.
impl std::fmt::Debug for DownloaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (variant, msg) = match self {
            DownloaderError::HttpFailure(m) => ("HttpFailure", m),
            DownloaderError::AuthFailed(m) => ("AuthFailed", m),
            DownloaderError::ApiError(m) => ("ApiError", m),
            DownloaderError::MalformedResponse(m) => ("MalformedResponse", m),
        };
        f.debug_tuple(variant)
            .field(&longbox_core::redact::secrets(msg))
            .finish()
    }
}

impl DownloaderError {
    /// True when retrying is futile (the user must intervene). Step 6
    /// uses this to choose between backoff-retry and the attention
    /// queue.
    pub fn is_permanent(&self) -> bool {
        matches!(self, DownloaderError::AuthFailed(_))
    }
}
