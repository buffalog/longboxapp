use thiserror::Error;

/// A downloader operation failure. As in `longbox-newznab`, the variant
/// carries the permanent-vs-transient signal so Step 6's retry table
/// (submission failure → exponential backoff) matches on it directly
/// rather than introspecting strings.
#[derive(Debug, Error)]
pub enum DownloaderError {
    /// Network failure, timeout, or non-success HTTP status (other
    /// than auth). Transient — worth a backoff retry.
    #[error("http request failed: {0}")]
    HttpFailure(String),

    /// The downloader rejected our credentials — bad SABnzbd apikey,
    /// bad NZBGet Basic-auth. Permanent: retrying won't help, the user
    /// must fix the config.
    #[error("downloader rejected credentials: {0}")]
    AuthFailed(String),

    /// The downloader accepted the request but reported an error
    /// (SAB `{"status": false, "error": ...}`, NZBGet JSON-RPC
    /// `error` envelope, an `append` returning a non-positive id).
    #[error("downloader reported an error: {0}")]
    ApiError(String),

    /// Response body didn't parse as the expected shape.
    #[error("malformed downloader response: {0}")]
    MalformedResponse(String),
}

impl DownloaderError {
    /// True when retrying is futile (the user must intervene). Step 6
    /// uses this to choose between backoff-retry and the attention
    /// queue.
    pub fn is_permanent(&self) -> bool {
        matches!(self, DownloaderError::AuthFailed(_))
    }
}
