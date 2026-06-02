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

    /// `sweep_single_series` was asked to run for a series that isn't
    /// on the pull list. Defensive: the route handler does its own 404
    /// preflight first, but a direct caller (test, future internal
    /// trigger) needs the typed signal too.
    #[error("series {series_id} is not on the pull list")]
    NotOnPullList { series_id: i64 },
}
