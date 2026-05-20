//! LongBox domain types and pure logic.
//!
//! This crate has no I/O, no HTTP, no SQL. It compiles standalone and every
//! test runs without any async runtime, database, or network. The matcher
//! algorithm (Tier 2 + Tier 3) takes a pre-fetched [`Candidate`] pool as a
//! slice; Tier 1 (`<Web>` URL → issue ID) is the caller's responsibility,
//! built from the per-URL extraction helpers in [`comicinfo`].

pub mod comicinfo;
pub mod comicinfo_writer;
pub mod error;
pub mod file;
pub mod filename;
pub mod issue;
pub mod library_path;
pub mod matcher;
pub mod normalize;
pub mod series;
pub mod similarity;

pub use comicinfo::{extract_cv_issue_id_from_url, extract_metron_issue_id_from_url, ComicInfo};
pub use comicinfo_writer::{ComicInfoMetadata, CoverDate};
pub use error::{CoreError, Result};
pub use file::{classify_status, FileStatus, LocalFile, MatchMethod};
pub use filename::{ParsedFilename, ParsingPattern};
pub use issue::{Issue, IssueNumber};
pub use library_path::LibraryPath;
pub use matcher::{match_file, Candidate, MatchResult};
pub use normalize::normalize_title;
pub use series::Series;

/// Confidence floor below which a tier produces no match and the matcher
/// falls through to the next tier.
pub const NEEDS_REVIEW_FLOOR: f64 = 0.65;

/// Confidence ceiling applied to Tier 3 (filename regex) matches. Even a
/// perfect filename match cannot exceed this, biasing the system toward
/// trusting embedded ComicInfo metadata over filenames.
pub const FILENAME_CONFIDENCE_CEILING: f64 = 0.90;

/// Default `owned`-vs-`needs_review` boundary. Storable in the `settings`
/// table; the matcher itself does not consult this — callers apply it
/// post-hoc to classify status from `MatchResult.confidence`.
pub const DEFAULT_MATCH_THRESHOLD: f64 = 0.85;

/// Free-function form of [`ComicInfo::parse`].
pub fn parse_comicinfo(xml: &[u8]) -> Result<ComicInfo> {
    ComicInfo::parse(xml)
}

/// Free-function form of [`filename::parse`].
pub fn parse_filename(filename: &str, patterns: &[ParsingPattern]) -> Option<ParsedFilename> {
    filename::parse(filename, patterns)
}
