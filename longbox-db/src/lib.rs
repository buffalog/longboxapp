//! LongBox SQLx pool, migrations, and repository layer.
//!
//! Each `*_repo` module exposes free async functions generic over
//! `sqlx::SqliteExecutor`, so callers pass `&SqlitePool` for single
//! statements or `&mut Transaction<'_, Sqlite>` for transactional work.

pub mod error;
pub mod file_repo;
pub mod issue_repo;
pub mod library_root_repo;
pub mod parsing_pattern_repo;
pub mod pool;
pub mod scan_run_repo;
pub mod series_repo;
pub mod settings_repo;

pub use error::{DbError, Result};
pub use file_repo::{FileRow, FileUpdate, NewFile};
pub use issue_repo::{IssueRow, IssueUpdate, NewIssue};
pub use library_root_repo::{LibraryRootRow, NewLibraryRoot};
pub use parsing_pattern_repo::{NewParsingPattern, ParsingPatternRow};
pub use pool::open;

/// Re-export of [`sqlx::SqlitePool`] under the project's canonical name.
/// Downstream crates depend on `longbox_db::Pool`, not `sqlx::SqlitePool`,
/// so that future re-implementations or pool wrappers don't ripple through
/// every caller.
pub type Pool = sqlx::SqlitePool;
pub use scan_run_repo::{NewScanRun, ScanProgress, ScanRunRow, ScanRunStatus};
pub use series_repo::{NewSeries, SeriesRow, SeriesUpdate};
pub use settings_repo::{KEY_LIBRARY_ROOT_PATH, KEY_MATCH_CONFIDENCE_THRESHOLD};
