use thiserror::Error;

/// Errors surfaced from the post-process pipeline.
///
/// Step 4 (skeleton) carries only the catalog-write path's error
/// surface. Watcher / I/O / ComicInfo write variants land in later
/// steps as their call paths arrive; the enum stays small in the
/// meantime so a panic on an unknown variant in match-anywhere code
/// would be impossible.
#[derive(Debug, Error)]
pub enum PostprocessError {
    #[error(transparent)]
    Db(#[from] longbox_db::DbError),
}

pub type Result<T> = std::result::Result<T, PostprocessError>;
