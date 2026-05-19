use thiserror::Error;

use crate::types::IndexerId;

/// A single indexer's failure. The variant *is* the permanent-vs-
/// transient signal — Step 6's retry table matches on it directly
/// rather than introspecting error strings.
#[derive(Debug, Error)]
pub enum IndexerError {
    /// Network failure, timeout, or non-success HTTP status. Transient
    /// — worth retrying on the next sweep.
    #[error("http request failed: {0}")]
    HttpFailure(String),

    /// Indexer rejected our API key / account (Newznab `<error>`
    /// codes 100-107, the account range). Permanent — retrying won't
    /// help; the user must fix the key. Surfaces to the attention
    /// queue.
    #[error("indexer rejected credentials (newznab error {code}): {description}")]
    BadCredentials { code: u32, description: String },

    /// Response didn't parse, or a non-credential Newznab `<error>`
    /// (codes 2xx/3xx/9xx — bad request, API disabled, etc.). The
    /// embedded message carries the detail. Treated as needs-
    /// investigation rather than cleanly transient or permanent.
    #[error("malformed indexer response: {0}")]
    MalformedResponse(String),
}

impl IndexerError {
    /// True when retrying is futile (the user must intervene). Step 6
    /// uses this to decide between scheduled-retry and attention-queue.
    pub fn is_permanent(&self) -> bool {
        matches!(self, IndexerError::BadCredentials { .. })
    }
}

/// Error from the high-level [`find_release`](crate::find_release)
/// search. The only failure mode is "every indexer failed" — a search
/// where indexers responded but found nothing is `Ok(None)`, not an
/// error.
#[derive(Debug, Error)]
pub enum NewznabError {
    /// Every indexer in the list errored. Carries each indexer's id +
    /// failure so the caller can tell permanent failures (bad key)
    /// from transient ones (timeout) without string-matching.
    #[error("all {} indexer(s) failed", .0.len())]
    AllIndexersFailed(Vec<(IndexerId, IndexerError)>),
}
