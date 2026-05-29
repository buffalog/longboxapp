//! LongBox Newznab indexer client.
//!
//! Searches Newznab-protocol indexers for comic NZB releases. Built for
//! Phase A.8's auto-pull workflow: given a series + issue, find the best
//! release across a priority-ordered list of indexers.
//!
//! Scope (Phase A.8 Step 1):
//! - Newznab `t=search` request construction (two query variations:
//!   zero-padded issue, then unpadded)
//! - RSS/XML response parsing with defensive `Option` discipline
//! - provider iteration: priority order, first-with-results wins
//! - result selection: cbz preferred, highest grabs, recency tie-break
//!
//! Not in scope: rate limiting (A.8+ deferral), torrent/Torznab support
//! (Usenet only — permanent scope exclusion).
//!
//! Entry point: [`find_release`]. [`search_indexer`] is the lower-level
//! single-indexer call.

mod client;
mod error;
mod parse;
mod query;
mod select;
mod types;

pub use client::{
    find_release, find_release_excluding, find_release_excluding_filtered, search_indexer,
    test_connection, FindOutcome,
};
pub use error::{IndexerError, NewznabError};
pub use select::{normalize_scene_title, FilterOutcome, MismatchDiagnostic};
pub use types::{ArchiveFormat, IndexerConfig, IndexerId, Release};
