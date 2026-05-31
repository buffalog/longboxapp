//! Public, flat types projected from the raw Metron DTOs. Downstream
//! callers (the web layer's calendar handler, primarily) depend on these
//! shapes — never on `reqwest`, never on the raw nested envelopes.
//!
//! Each projection function takes ownership of the raw struct and returns
//! the projected one; no clones, no `Option` chains for fields the API
//! always supplies.

use serde::{Deserialize, Serialize};

use crate::models::{MetronIssueDetailRow, MetronIssueListRow, MetronSeriesDetailRow,
    MetronSeriesListRow};

/// One forward-calendar entry from Metron's `store_date_range_*` query.
/// The shape mirrors `longbox-comicvine::CvCalendarItem` plus Metron-
/// specific fields (`metron_issue_id`, `metron_series_id`, `foc_date`).
///
/// **`publisher` is `None` on a list-fetch projection** — Metron's list
/// endpoint doesn't carry publisher. Piece 3 hydrates this by joining on
/// series-detail fetches before returning the row to the web layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetronCalendarItem {
    /// Metron's own issue id — the load-bearing cross-reference for the
    /// `cv_volume_id`-keyed model's `metron_id` field on `CalendarRow`.
    pub metron_issue_id: i64,
    /// Metron's series id. `None` on list-projection (the list endpoint
    /// omits it from the embedded series block); populated on detail-
    /// projection.
    pub metron_series_id: Option<i64>,
    pub issue_number: String,
    pub store_date: Option<String>,
    pub cover_date: Option<String>,
    /// Final Order Cutoff — the date past which retailers can't adjust
    /// their order quantity. Useful for "shipping soon" UI signals.
    pub foc_date: Option<String>,
    pub series_name: String,
    pub series_volume: i64,
    pub series_year_began: i64,
    pub cover_url: Option<String>,
    /// Hydrated by piece 3's publisher-hydration path — `None` on a fresh
    /// list-projection.
    pub publisher: Option<String>,
    /// CV's issue-level id, when Metron has it. `None` for forward issues
    /// CV hasn't catalogued yet (most of the forward-window content).
    pub cv_issue_id: Option<i64>,
    pub site_detail_url: Option<String>,
}

/// Volume detail projection used for piece 3's publisher-by-series
/// hydration and piece 4's cv_volume_id → metron_series_id backfill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetronSeriesDetail {
    pub metron_series_id: i64,
    pub name: String,
    pub volume: i64,
    pub year_began: i64,
    pub year_end: Option<i64>,
    pub publisher: Option<String>,
    /// CV's volume-level id (matches our `series.cv_id` column).
    pub cv_id: Option<i64>,
}

/// One row from the `?cv_id=` lookup used to map LongBox's CV-keyed
/// series to Metron's series id. Piece 4 uses this for backfilling
/// `series.metron_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetronSeriesRef {
    pub metron_series_id: i64,
    /// Pre-formatted "Saga (2012)" style label from Metron.
    pub display_name: String,
    pub year_began: i64,
    pub year_end: Option<i64>,
    pub volume: i64,
    pub issue_count: i64,
}

// -------- projection functions --------

pub(crate) fn project_calendar_item(raw: MetronIssueListRow) -> MetronCalendarItem {
    MetronCalendarItem {
        metron_issue_id: raw.id,
        // List endpoint omits series.id — piece 3's hydration choice
        // determines how this gets populated (re-fetch via detail, or
        // dedup by name+volume+year_began, or accept None and key by
        // metron_issue_id alone).
        metron_series_id: None,
        issue_number: raw.number,
        store_date: raw.store_date,
        cover_date: raw.cover_date,
        // List endpoint also omits foc_date — only available on detail.
        foc_date: None,
        series_name: raw.series.name,
        series_volume: raw.series.volume,
        series_year_began: raw.series.year_began,
        cover_url: raw.image,
        publisher: None,
        cv_issue_id: None,
        site_detail_url: None,
    }
}

pub(crate) fn project_calendar_item_from_detail(raw: MetronIssueDetailRow) -> MetronCalendarItem {
    let publisher = raw.publisher.map(|p| p.name);
    MetronCalendarItem {
        metron_issue_id: raw.id,
        metron_series_id: Some(raw.series.id),
        issue_number: raw.number,
        store_date: raw.store_date,
        cover_date: raw.cover_date,
        foc_date: raw.foc_date,
        series_name: raw.series.name,
        series_volume: raw.series.volume,
        series_year_began: raw.series.year_began,
        cover_url: raw.image,
        publisher,
        cv_issue_id: raw.cv_id,
        site_detail_url: raw.resource_url,
    }
}

pub(crate) fn project_series_detail(raw: MetronSeriesDetailRow) -> MetronSeriesDetail {
    MetronSeriesDetail {
        metron_series_id: raw.id,
        name: raw.name,
        volume: raw.volume,
        year_began: raw.year_began,
        year_end: raw.year_end,
        publisher: raw.publisher.map(|p| p.name),
        cv_id: raw.cv_id,
    }
}

pub(crate) fn project_series_ref(raw: MetronSeriesListRow) -> MetronSeriesRef {
    MetronSeriesRef {
        metron_series_id: raw.id,
        display_name: raw.series,
        year_began: raw.year_began,
        year_end: raw.year_end,
        volume: raw.volume,
        issue_count: raw.issue_count,
    }
}
