//! Raw Metron API DTOs. These types mirror the JSON wire format closely;
//! the projection layer ([`crate::projection`]) maps them to flat, public
//! types that downstream callers consume.
//!
//! Field shapes are based on empirical probes from the Item A v2
//! archaeology pass (2026-05-31). A few fields are modeled as `Option`
//! defensively because Metron is not strict about which fields are
//! present on every record — older issues may lack `foc_date`, brand-new
//! ones may lack `cv_id` until CV catalogs them, etc.

use serde::Deserialize;

/// Generic pagination envelope used by every list endpoint.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronList<T> {
    pub count: i64,
    pub next: Option<String>,
    #[allow(dead_code)]
    pub previous: Option<String>,
    pub results: Vec<T>,
}

/// One row from `GET /api/issue/?store_date_range_after=...&...=...`.
///
/// The list endpoint is the cheapest forward-calendar fetch — one HTTP
/// call returns the full week's issues, with the embedded series carrying
/// enough metadata to display (name, volume, year_began) BUT no `id`,
/// `cv_id`, or `publisher`. Those require either an issue-detail fetch
/// per row or a series-detail fetch per unique series — the publisher-
/// hydration strategy is piece-3 work.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronIssueListRow {
    pub id: i64,
    pub series: MetronEmbeddedSeriesLite,
    pub number: String,
    #[allow(dead_code)]
    pub issue: Option<String>,
    pub cover_date: Option<String>,
    pub store_date: Option<String>,
    pub image: Option<String>,
    #[allow(dead_code)]
    pub cover_hash: Option<String>,
    #[allow(dead_code)]
    pub modified: Option<String>,
}

/// Embedded series block on a list-endpoint issue. Notably MISSING `id`
/// (which is only on the detail-endpoint series block — this is the gap
/// that drives piece 3's publisher-hydration design choice).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronEmbeddedSeriesLite {
    pub name: String,
    pub volume: i64,
    pub year_began: i64,
}

/// `GET /api/issue/{id}/` — the heavy-detail endpoint. Has publisher
/// inline, the series block with `id` for cross-reference, FOC date,
/// and the CV / GCD cross-references on the issue itself.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronIssueDetailRow {
    pub id: i64,
    pub publisher: Option<MetronPublisher>,
    pub series: MetronEmbeddedSeriesFull,
    pub number: String,
    pub cover_date: Option<String>,
    pub store_date: Option<String>,
    pub foc_date: Option<String>,
    pub image: Option<String>,
    /// CV's issue-level id (Metron's cross-reference). Often `None` for
    /// brand-new forward issues that CV hasn't catalogued yet.
    pub cv_id: Option<i64>,
    pub resource_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronEmbeddedSeriesFull {
    pub id: i64,
    pub name: String,
    pub volume: i64,
    pub year_began: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronPublisher {
    /// Metron's publisher id. Currently unused (the projection layer only
    /// surfaces `name`), but preserved on the DTO for future cross-
    /// reference work — e.g., a publisher-filter table that joins on
    /// Metron's stable publisher id rather than the display name.
    #[allow(dead_code)]
    pub id: i64,
    pub name: String,
}

/// `GET /api/series/{id}/` — series detail, which carries `publisher`
/// inline as well. Used by piece 3's hydration when fetching publisher
/// per unique series rather than per issue.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronSeriesDetailRow {
    pub id: i64,
    pub name: String,
    pub volume: i64,
    pub year_began: i64,
    pub year_end: Option<i64>,
    pub publisher: Option<MetronPublisher>,
    /// CV's volume-level id (the same shape as our `series.cv_id`).
    pub cv_id: Option<i64>,
    /// Run status — `Completed | Cancelled | Ongoing | Hiatus` (added to
    /// the Metron Series model June 2024). Optional/defaulted so older
    /// records or a schema change can't break deserialization. Projected
    /// to the `finished` flag via [`super::projection::finished_from_status`].
    #[serde(default)]
    pub status: Option<String>,
}

/// One row from `GET /api/series/?cv_id={id}` — the reverse-mapping
/// endpoint LongBox uses to resolve its CV-linked series to Metron's
/// series id. Piece 4 builds the cv_volume_id → metron_series_id map
/// from this.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetronSeriesListRow {
    pub id: i64,
    /// Pre-formatted display name like "Saga (2012)".
    pub series: String,
    pub year_began: i64,
    pub year_end: Option<i64>,
    pub volume: i64,
    pub issue_count: i64,
}
