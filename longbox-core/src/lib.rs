//! LongBox domain types and pure logic.
//!
//! This crate has no I/O, no HTTP, no SQL. It compiles standalone and every
//! test runs without any async runtime, database, or network. The matcher
//! algorithm takes its candidate series and issue pools as slices; the caller
//! is responsible for fetching them.

pub mod comicinfo;
pub mod error;
pub mod file;
pub mod filename;
pub mod issue;
pub mod matcher;
pub mod normalize;
pub mod series;
pub mod similarity;

pub use comicinfo::ComicInfo;
pub use error::{CoreError, Result};
pub use file::{classify_status, FileStatus, LocalFile, MatchMethod};
pub use filename::{ParsedFilename, ParsingPattern};
pub use issue::{Issue, IssueNumber};
pub use matcher::{match_file, FileContext, MatchResult};
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
