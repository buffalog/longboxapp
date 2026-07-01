//! LongBox shallow-series CV enrichment worker (Phase A.9 Step 6c.2).
//!
//! Continuous-loop worker that promotes shallow-converted series
//! (`cv_id IS NULL`) to CV-linked by auto-picking a ComicVine volume
//! per the discipline in [`longbox_comicvine::enrichment::pick_volume`]
//! (the 6c.1 pure logic) and merging the CV issue list via the
//! row-id-preserving
//! [`longbox_db::issue_repo::upsert_by_series_id_and_number_with_cv_fields`].
//!
//! **Crash-safety property.** Per-series enrichment uses the
//! fetch-then-atomic-write shape pioneered by `series::refresh`:
//! network calls (`fetch_volume`, `fetch_issues`) happen *outside* a
//! transaction; all DB writes (`set_cv_id`, the per-issue upserts,
//! the outcome record) happen *inside* a single transaction. Task
//! cancellation during fetch → no DB writes happened, series stays
//! shallow, next cycle retries cleanly. Task cancellation
//! mid-transaction → SQLite rolls back, no partial writes survive.
//! The transaction boundary IS the recovery point; no torn
//! intermediate state can exist.
//!
//! **Two independent refusal nets** carried over from 6c.1:
//! 1. Catalog-collision pre-filter (knowable from catalog state) —
//!    fires upstream of CV calls for year-unknown rows with ≥ 2
//!    same-titled siblings. Records `collision_disabled` as an
//!    explicit positive outcome (loudness-not-silence: proves the
//!    pre-filter ran).
//! 2. Runtime dominant-gap guard (kicks in after CV responds) —
//!    refuses when two candidates compete.
//!
//! **Background priority.** CV calls from the worker go through
//! [`background::BackgroundCvClient`], a layered rate limiter that
//! enforces 30s spacing on background callers in front of the
//! shared 180/h limiter. Interactive callers (refresh, add, search)
//! continue to use the bare client and get the original
//! next-slot latency — they do not queue behind background tokens.

pub mod background;
pub mod credits_resolver;
pub mod config;
pub mod outcome;
pub mod worker;

pub use background::BackgroundCvClient;
pub use config::EnrichmentConfig;
pub use outcome::AttemptOutcome;
pub use worker::{spawn, EnrichmentHandle};
