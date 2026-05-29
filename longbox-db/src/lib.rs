//! LongBox SQLx pool, migrations, and repository layer.
//!
//! Each `*_repo` module exposes free async functions generic over
//! `sqlx::SqliteExecutor`, so callers pass `&SqlitePool` for single
//! statements or `&mut Transaction<'_, Sqlite>` for transactional work.

pub mod candidate;
pub mod discovered_folders_repo;
pub mod downloader_config_repo;
pub mod error;
pub mod file_repo;
pub mod indexer_config_repo;
pub mod issue_repo;
pub mod library_root_repo;
pub mod parsing_pattern_repo;
pub mod pool;
pub mod publisher_filter_repo;
pub mod pull_attempt_repo;
pub mod pull_list_repo;
pub mod release_cache_repo;
pub mod scan_run_repo;
pub mod series_repo;
pub mod settings_repo;
pub mod webhook_config_repo;

pub use candidate::{find_candidates, row_to_issue, row_to_series};
pub use discovered_folders_repo::{DiscoveredFolder, DiscoveredFolderRow};
pub use downloader_config_repo::{DownloaderConfigRow, NewDownloaderConfig};
pub use error::{DbError, Result};
pub use file_repo::{FileRow, FileUpdate, NewFile};
pub use indexer_config_repo::{IndexerConfigRow, IndexerConfigUpdate, NewIndexerConfig};
pub use issue_repo::{IssueRow, IssueUpdate, NewIssue};
pub use library_root_repo::{LibraryRootRow, NewLibraryRoot};
pub use parsing_pattern_repo::{NewParsingPattern, ParsingPatternRow};
pub use pool::open;
pub use publisher_filter_repo::{
    NewPublisherFilter, PublisherFilterMode, PublisherFilterRow, DEFAULT_BLOCKED_PUBLISHERS,
};
pub use pull_attempt_repo::{FailedPull, NewPullAttempt, PullAttemptRow};
pub use pull_list_repo::{NewPullEntry, PullListRow, PullListWithSeries};
pub use release_cache_repo::{NewReleaseCacheEntry, ReleaseCacheRow};
pub use webhook_config_repo::{NewWebhookConfig, WebhookConfigRow, WebhookConfigUpdate};

/// Re-export of [`sqlx::SqlitePool`] under the project's canonical name.
/// Downstream crates depend on `longbox_db::Pool`, not `sqlx::SqlitePool`,
/// so that future re-implementations or pool wrappers don't ripple through
/// every caller.
pub type Pool = sqlx::SqlitePool;
pub use scan_run_repo::{
    NewScanRun, ScanCompletion, ScanProgress, ScanRunKind, ScanRunRow, ScanRunStatus,
};
pub use series_repo::{
    CandidateSelectionMode, NewSeries, PhantomSeries, SeriesRow, SeriesUpdate, SeriesWithCounts,
    ShallowEnrichmentCandidate,
};
pub use settings_repo::{KEY_LIBRARY_ROOT_PATH, KEY_MATCH_CONFIDENCE_THRESHOLD};
