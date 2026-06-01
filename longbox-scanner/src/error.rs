use std::path::PathBuf;

use thiserror::Error;

/// Errors surfaced from the scanner crate. Whole-scan failures abort with
/// `ScanError`; per-file failures are collected into
/// [`crate::ScanReport::errors`] and the scan continues.
#[derive(Debug, Error)]
pub enum ScanError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("db error: {0}")]
    Db(#[from] longbox_db::DbError),

    /// Wraps `walkdir::Error` as a string because the original type does not
    /// implement `Clone`/`Send` cleanly across `Result` boundaries.
    #[error("directory traversal error: {0}")]
    Walk(String),

    #[error("library root id {id} not found")]
    LibraryRootNotFound { id: i64 },

    /// Another scan holds the per-scanner lock. Callers should treat this as
    /// "try again later"; the crate deliberately does NOT block.
    #[error("another scan is already running")]
    AlreadyRunning,

    #[error("invalid path {path:?}: {reason}")]
    InvalidPath { path: PathBuf, reason: String },

    /// Mount-health guard tripped: the library root is unreadable or
    /// empty. Returned BEFORE the scan_run is recorded and BEFORE any
    /// destructive pass (mark-missing, auto-tidy) can run, so a
    /// degraded SMB mount can never silently flip the catalog to
    /// "everything is gone" and trigger auto-tidy purges.
    #[error("scan preflight failed: {reason}")]
    PreflightFailed { reason: String },
}
