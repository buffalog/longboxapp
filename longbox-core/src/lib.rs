//! LongBox domain types and pure logic.
//!
//! This crate has no I/O, no HTTP, no SQL. It compiles standalone and every
//! test runs without any async runtime, database, or network. The matcher
//! algorithm (Tier 2 + Tier 3) takes a pre-fetched [`Candidate`] pool as a
//! slice; Tier 1 (`<Web>` URL → issue ID) is the caller's responsibility,
//! built from the per-URL extraction helpers in [`comicinfo`].

pub mod archive_label;
pub mod comicinfo;
pub mod comicinfo_writer;
pub mod error;
pub mod file;
pub mod filename;
pub mod issue;
pub mod library_path;
pub mod matcher;
pub mod metroninfo_writer;
pub mod normalize;
pub mod series;
pub mod similarity;

pub use comicinfo::{extract_cv_issue_id_from_url, extract_metron_issue_id_from_url, ComicInfo};
pub use comicinfo_writer::ComicInfoMetadata;
pub use error::{CoreError, Result};
pub use file::{classify_status, FileStatus, LocalFile, MatchMethod};
pub use filename::{
    normalize_dotted_basename, parse_with_normalization, ParsedFilename, ParsingPattern,
};
pub use issue::{Issue, IssueNumber};
pub use library_path::LibraryPath;
pub use matcher::{match_file, Candidate, MatchResult};
pub use metroninfo_writer::MetronInfoMetadata;
pub use normalize::normalize_title;
pub use series::Series;

/// Confidence floor below which a tier produces no match and the matcher
/// falls through to the next tier.
pub const NEEDS_REVIEW_FLOOR: f64 = 0.65;

/// Confidence ceiling applied to Tier 3 (filename regex) matches. Even a
/// perfect filename match cannot exceed this, biasing the system toward
/// trusting embedded ComicInfo metadata over filenames.
pub const FILENAME_CONFIDENCE_CEILING: f64 = 0.90;

/// SEED-DEFAULT for the `owned`-vs-`needs_review` boundary. The DB
/// `settings.match_confidence_threshold` row is the single source of
/// truth at runtime — the scanner reads it per scan run, Phase B
/// reads it per sweep, the Settings UI exposes it for editing. This
/// constant fires ONLY when the row is absent or unparseable (first
/// boot, pre-migration state), and never silently overrides a saved
/// value. The matcher itself does not consult this — callers apply
/// it post-hoc to classify status from `MatchResult.confidence`.
pub const DEFAULT_MATCH_THRESHOLD: f64 = 0.85;

/// SEED-DEFAULT for the pull engine's pre-grab newznab series-title
/// filter (Bug 3). The DB `settings.pull_indexer_match_threshold`
/// row is the single source of truth at runtime — the engine reads
/// it per sweep, the Settings UI exposes it for editing. Stricter
/// than the catalog matcher's `NEEDS_REVIEW_FLOOR` because pre-grab
/// errors are asymmetrically costly: a wrong NZB consumes bandwidth,
/// churns SAB, and pollutes the catalog when Phase B imports it,
/// while a missed legitimate match is a visible, recoverable park.
/// Calibrated at the Wolverine-MAX boundary (`wolverine` vs
/// `wolverine max` scores ~0.69, which 0.65 wrongly accepts and
/// 0.75 correctly rejects). Falls back to this constant only when
/// the settings row is absent / unparseable.
pub const PULL_INDEXER_MATCH_THRESHOLD: f64 = 0.75;

/// Free-function form of [`ComicInfo::parse`].
pub fn parse_comicinfo(xml: &[u8]) -> Result<ComicInfo> {
    ComicInfo::parse(xml)
}

/// Free-function form of [`filename::parse`].
pub fn parse_filename(filename: &str, patterns: &[ParsingPattern]) -> Option<ParsedFilename> {
    filename::parse(filename, patterns)
}

/// Free-function form of [`filename::parse_with_normalization`].
/// Phase B's per-file processor calls this so dot-separated NZB-style
/// basenames (`Absolute.Green.Lantern.007.(2025).cbz`) parse via the
/// existing strict patterns instead of dead-ending at `_unsorted/`.
pub fn parse_filename_with_normalization(
    filename: &str,
    patterns: &[ParsingPattern],
) -> Option<ParsedFilename> {
    filename::parse_with_normalization(filename, patterns)
}
