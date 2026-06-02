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

    /// `sweep_single_issue` was asked to run for a series id that has
    /// no `series` row. The series-detail-page Search button cannot
    /// produce this for live data; defensive for direct callers.
    #[error("series {series_id} not found")]
    SeriesNotFound { series_id: i64 },

    /// `sweep_single_issue` was asked to run for an issue id that has
    /// no `issues` row.
    #[error("issue {issue_id} not found")]
    IssueNotFound { issue_id: i64 },

    /// The issue's `series_id` doesn't match the series id in the
    /// request path — URL tampering or a stale UI. Surfaced as a 404
    /// scoped to the issue resource so callers see a clean "this
    /// issue isn't part of that series" signal rather than a 500.
    #[error("issue {issue_id} does not belong to series {series_id} (actually belongs to series {actual_series_id})")]
    IssueSeriesMismatch {
        series_id: i64,
        issue_id: i64,
        actual_series_id: i64,
    },
}
