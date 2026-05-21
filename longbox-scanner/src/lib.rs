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

mod walker;

pub use error::ScanError;
pub use report::{ScanFileError, ScanReport};
pub use scanner::{Scanner, ScannerConfig};
