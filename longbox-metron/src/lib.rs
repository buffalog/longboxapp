//! Metron API client. Sister crate to `longbox-comicvine`; owns rate
//! limiting, retry policy, and projection from Metron's nested JSON shape
//! to flat, public types.
//!
//! Callers never see `reqwest`. Public surface is intentionally narrow:
//! [`MetronClient`], [`MetronClientConfig`], [`MetronError`], and the
//! three projected types — [`MetronCalendarItem`], [`MetronSeriesDetail`],
//! [`MetronSeriesRef`].
//!
//! This is piece 1 of the Item A v2 work. Catalog integration (route
//! handler, settings rows, kill switch, frontend wiring) is piece 2-3,
//! `series.metron_id` backfill is piece 4.

pub mod client;
pub mod error;
pub mod projection;

mod models;
mod rate_limit;

pub use client::{MetronClient, MetronClientConfig};
pub use error::MetronError;
pub use projection::{MetronCalendarItem, MetronIssueRef, MetronSeriesDetail, MetronSeriesRef};
