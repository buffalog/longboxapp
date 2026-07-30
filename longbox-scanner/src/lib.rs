//! LongBox library scanner. Walks the disk, reads ComicInfo.xml from CBZ
//! and CBR archives, runs the three-tier matcher cascade against each
//! file, and persists results via `longbox-db`.
//!
//! Tier 1 (`<Web>` URL → DB issue ID) lives in [`scanner`] because it needs
//! DB access. Tiers 2 + 3 (similarity matching) are delegated to
//! [`longbox_core::match_file`] with pre-fetched candidates.
//!
//! The crate has no `longbox-comicvine` dependency — it is fully offline.

pub mod error;
pub mod report;
pub mod scanner;

/// Public so Library Integrity's disk/DB reconciliation can walk with the
/// SAME rules the scanner catalogues by. Reimplementing the walk elsewhere
/// would drift — a file the scanner skips but the reconciler sees becomes a
/// permanent false "orphan".
pub mod walker;

pub use error::ScanError;
pub use report::{ScanFileError, ScanReport};
pub use scanner::{Scanner, ScannerConfig};
pub use walker::{walk_library, DiscoveredFile};
