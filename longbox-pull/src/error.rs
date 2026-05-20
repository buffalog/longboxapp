//! Pull engine errors.

use thiserror::Error;

/// A pull-engine failure.
///
/// Indexer and downloader failures are deliberately *not* `PullError`s:
/// they are recorded as `pull_attempts` row states and drive the retry
/// logic. `PullError` is reserved for failures that abort a sweep
/// outright — principally database errors.
#[derive(Debug, Error)]
pub enum PullError {
    #[error(transparent)]
    Db(#[from] longbox_db::DbError),
}
