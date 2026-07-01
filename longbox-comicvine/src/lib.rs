//! ComicVine API client. Owns rate limiting, retry policy, and projection
//! from CV's nested JSON shape to flat, public types.
//!
//! Callers never see `reqwest`. Public surface is intentionally narrow:
//! [`ComicVineClient`], [`ComicVineClientConfig`], [`CvError`], and the three
//! projected types — [`SeriesSearchResult`], [`CvVolumeDetail`],
//! [`CvIssueDetail`].

pub mod client;
pub mod enrichment;
pub mod error;
pub mod projection;

mod models;
mod rate_limit;

pub use client::{ComicVineClient, ComicVineClientConfig, CvRateLimitSnapshot};
pub use enrichment::{pick_volume, EnrichmentCandidateInput, EnrichmentThresholds, PickOutcome};
pub use error::CvError;
pub use projection::{CvCalendarItem, CvIssueDetail, CvPersonCredit, CvVolumeDetail, SeriesSearchResult};
