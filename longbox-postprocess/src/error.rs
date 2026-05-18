use std::path::PathBuf;

use thiserror::Error;

/// Errors surfaced from the post-process pipeline.
///
/// Variants land step-by-step as their call paths arrive — the enum
/// stays honest about what can actually go wrong at any given commit.
/// Step 5 adds `WatchPathUnreadable` (configured directory missing /
/// not accessible at startup) and `WatcherInit` (notify failed to set
/// up the OS-level subscription).
#[derive(Debug, Error)]
pub enum PostprocessError {
    #[error(transparent)]
    Db(#[from] longbox_db::DbError),

    #[error("watch path {path:?} is not a readable directory: {source}")]
    WatchPathUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("filesystem watcher init failed: {0}")]
    WatcherInit(#[from] notify::Error),
}

pub type Result<T> = std::result::Result<T, PostprocessError>;
