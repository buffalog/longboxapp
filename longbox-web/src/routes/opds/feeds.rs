//! OPDS navigation feed handlers: catalog root, all-series, publisher
//! list, and per-publisher series. Each reads the local library via
//! `longbox_db::opds_repo`, builds a pure [`longbox_opds::Feed`] with
//! absolute hrefs rooted at `config.opds_base_url`, and renders it to Atom
//! XML with the OPDS navigation content-type.
//!
//! Acquisition feeds (issue lists, search results) are added in a later
//! commit; the series entries here already point at their future
//! acquisition endpoints (`/opds/v1/series/{id}`).

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use longbox_db::opds_repo::{self, OpdsIssue};
use longbox_db::series_repo::{self, SeriesRow};
use longbox_opds::{
    Entry, Feed, FeedKind, Link, Page, PageMeta, ACQUISITION_TYPE, ITEMS_PER_PAGE, NAVIGATION_TYPE,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;

use crate::error::ApiError;
use crate::state::AppState;

/// `?page=N`, 1-indexed; absent ⇒ page 1.
#[derive(Debug, Deserialize)]
pub struct PageQuery {
    page: Option<u64>,
}

impl PageQuery {
    fn number(&self) -> u64 {
        self.page.unwrap_or(1)
    }
}

/// `?q=...&page=N` for the search endpoint.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    page: Option<u64>,
}

impl SearchQuery {
    fn number(&self) -> u64 {
        self.page.unwrap_or(1)
    }
}

/// OPDS relation IRIs for the per-issue cover and download links.
const IMAGE_REL: &str = "http://opds-spec.org/image";
const ACQUISITION_REL: &str = "http://opds-spec.org/acquisition";

// ---------- handlers ----------

/// `GET /opds/v1` — the catalog root (navigation): links to the all-series
/// and by-publisher feeds.
pub async fn root(State(state): State<AppState>) -> Response {
    let base = &state.config.opds_base_url;
    let now = now_rfc3339();

    let feed = Feed {
        id: "urn:longbox:opds:root".to_owned(),
        title: "LongBox".to_owned(),
        updated: now.clone(),
        kind: FeedKind::Navigation,
        links: vec![
            self_link(base, "/opds/v1", FeedKind::Navigation),
            start_link(base),
            search_descriptor_link(base),
        ],
        pagination: None,
        entries: vec![
            Entry {
                id: "urn:longbox:opds:series".to_owned(),
                title: "All Series".to_owned(),
                updated: now.clone(),
                content: Some("Browse the full library by series.".to_owned()),
                links: vec![Link::new(
                    "subsection",
                    abs(base, "/opds/v1/series"),
                    NAVIGATION_TYPE,
                )],
                ..Default::default()
            },
            Entry {
                id: "urn:longbox:opds:publishers".to_owned(),
                title: "Publishers".to_owned(),
                updated: now,
                content: Some("Browse series grouped by publisher.".to_owned()),
                links: vec![Link::new(
                    "subsection",
                    abs(base, "/opds/v1/publishers"),
                    NAVIGATION_TYPE,
                )],
                ..Default::default()
            },
        ],
    };

    navigation_response(feed)
}

/// `GET /opds/v1/series` — all series, paginated (navigation).
pub async fn series_list(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let total = opds_repo::count_series(&state.db).await? as u64;
    let page = Page::new(query.number(), ITEMS_PER_PAGE, total);
    let rows =
        opds_repo::list_series_page(&state.db, page.limit() as i64, page.offset() as i64).await?;

    let base = &state.config.opds_base_url;
    let feed = series_feed(
        base,
        "urn:longbox:opds:series",
        "All Series",
        "/opds/v1/series",
        page,
        &rows,
        // Parent of the all-series feed is the catalog root.
        "/opds/v1",
    );
    Ok(navigation_response(feed))
}

/// `GET /opds/v1/publishers` — distinct publishers, paginated (navigation).
pub async fn publishers_list(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let total = opds_repo::count_publishers(&state.db).await? as u64;
    let page = Page::new(query.number(), ITEMS_PER_PAGE, total);
    let publishers =
        opds_repo::list_publishers_page(&state.db, page.limit() as i64, page.offset() as i64)
            .await?;

    let base = &state.config.opds_base_url;
    let path = "/opds/v1/publishers";
    let now = now_rfc3339();

    let entries = publishers
        .into_iter()
        .map(|p| {
            let encoded = utf8_percent_encode(&p.name, NON_ALPHANUMERIC).to_string();
            Entry {
                id: format!("urn:longbox:publisher:{encoded}"),
                title: p.name.clone(),
                updated: now.clone(),
                content: Some(plural_count(p.series_count, "series", "series")),
                links: vec![Link::new(
                    "subsection",
                    abs(base, &format!("/opds/v1/publishers/{encoded}/series")),
                    NAVIGATION_TYPE,
                )],
                ..Default::default()
            }
        })
        .collect();

    let feed = Feed {
        id: "urn:longbox:opds:publishers".to_owned(),
        title: "Publishers".to_owned(),
        updated: now,
        kind: FeedKind::Navigation,
        links: paginated_links(base, path, page, "/opds/v1", FeedKind::Navigation),
        pagination: Some(page_meta(page)),
        entries,
    };
    Ok(navigation_response(feed))
}

/// `GET /opds/v1/publishers/{name}/series` — series for one publisher,
/// paginated (navigation). `{name}` is axum-decoded from the percent-encoded
/// path segment.
pub async fn publisher_series(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let total = opds_repo::count_series_by_publisher(&state.db, &name).await? as u64;
    let page = Page::new(query.number(), ITEMS_PER_PAGE, total);
    let rows = opds_repo::list_series_by_publisher_page(
        &state.db,
        &name,
        page.limit() as i64,
        page.offset() as i64,
    )
    .await?;

    let base = &state.config.opds_base_url;
    let encoded = utf8_percent_encode(&name, NON_ALPHANUMERIC).to_string();
    let path = format!("/opds/v1/publishers/{encoded}/series");
    let feed = series_feed(
        base,
        &format!("urn:longbox:publisher:{encoded}:series"),
        &name,
        &path,
        page,
        &rows,
        // Parent of a per-publisher series feed is the publisher list.
        "/opds/v1/publishers",
    );
    Ok(navigation_response(feed))
}

/// `GET /opds/v1/series/{id}` — issues for one series, paginated
/// (acquisition). 404 if the series doesn't exist.
pub async fn series_detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let series = series_repo::find_by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        })?;

    let total = opds_repo::count_series_issues(&state.db, id).await? as u64;
    let page = Page::new(query.number(), ITEMS_PER_PAGE, total);
    let issues = opds_repo::list_series_issues_page(
        &state.db,
        id,
        page.limit() as i64,
        page.offset() as i64,
    )
    .await?;

    let base = &state.config.opds_base_url;
    let title = match series.start_year {
        Some(year) => format!("{} ({year})", series.title),
        None => series.title.clone(),
    };
    let path = format!("/opds/v1/series/{id}");
    let entries = issues.iter().map(|i| issue_entry(base, i)).collect();

    let feed = Feed {
        id: format!("urn:longbox:series:{id}:issues"),
        title,
        updated: now_rfc3339(),
        kind: FeedKind::Acquisition,
        // Parent of an issue list is the all-series feed.
        links: paginated_links(base, &path, page, "/opds/v1/series", FeedKind::Acquisition),
        pagination: Some(page_meta(page)),
        entries,
    };
    Ok(acquisition_response(feed))
}

/// `GET /opds/v1/search?q=...` — series whose title matches, paginated. A
/// navigation feed: each entry is a series linking to its acquisition feed.
/// An empty/absent `q` yields an empty feed rather than the whole library.
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Response, ApiError> {
    let raw = query.q.clone().unwrap_or_default();
    let term = raw.trim();
    let base = &state.config.opds_base_url;
    let q_enc = utf8_percent_encode(term, NON_ALPHANUMERIC).to_string();

    let total = if term.is_empty() {
        0
    } else {
        opds_repo::count_search_series(&state.db, term).await? as u64
    };
    let page = Page::new(query.number(), ITEMS_PER_PAGE, total);

    let entries = if term.is_empty() {
        Vec::new()
    } else {
        let rows = opds_repo::search_series_page(
            &state.db,
            term,
            page.limit() as i64,
            page.offset() as i64,
        )
        .await?;
        rows.iter().map(|s| series_entry(base, s)).collect()
    };

    let feed = Feed {
        id: "urn:longbox:opds:search".to_owned(),
        title: if term.is_empty() {
            "Search".to_owned()
        } else {
            format!("Search: {term}")
        },
        updated: now_rfc3339(),
        kind: FeedKind::Navigation,
        links: search_links(base, &q_enc, page),
        pagination: Some(page_meta(page)),
        entries,
    };
    Ok(navigation_response(feed))
}

/// `GET /opds/v1/opensearch.xml` — the OpenSearch descriptor. Unpaginated,
/// public-within-OPDS (still behind the auth layer).
pub async fn opensearch(State(state): State<AppState>) -> Response {
    let xml = longbox_opds::opensearch::descriptor(&state.config.opds_base_url);
    (
        [(
            header::CONTENT_TYPE,
            longbox_opds::opensearch::OPENSEARCH_DESCRIPTION_TYPE,
        )],
        xml,
    )
        .into_response()
}

// ---------- feed construction helpers ----------

/// Build a navigation feed whose entries are series, each linking to its
/// (future) acquisition feed at `/opds/v1/series/{id}`.
fn series_feed(
    base: &str,
    id: &str,
    title: &str,
    self_path: &str,
    page: Page,
    rows: &[SeriesRow],
    up_path: &str,
) -> Feed {
    let entries = rows.iter().map(|s| series_entry(base, s)).collect();
    Feed {
        id: id.to_owned(),
        title: title.to_owned(),
        updated: now_rfc3339(),
        kind: FeedKind::Navigation,
        links: paginated_links(base, self_path, page, up_path, FeedKind::Navigation),
        pagination: Some(page_meta(page)),
        entries,
    }
}

fn series_entry(base: &str, s: &SeriesRow) -> Entry {
    let title = match s.start_year {
        Some(year) => format!("{} ({year})", s.title),
        None => s.title.clone(),
    };
    // Short descriptive line: publisher when known, else the stored
    // description, else nothing.
    let content = s
        .publisher
        .clone()
        .filter(|p| !p.is_empty())
        .or_else(|| s.description.clone());

    Entry {
        id: format!("urn:longbox:series:{}", s.id),
        title,
        updated: rfc3339(s.updated_at),
        content,
        links: vec![Link::new(
            "subsection",
            abs(base, &format!("/opds/v1/series/{}", s.id)),
            // Target is the issue acquisition feed.
            ACQUISITION_TYPE,
        )],
        ..Default::default()
    }
}

/// Build an acquisition entry for one issue. The cover (image) link is
/// emitted only when a cover URL exists, and the acquisition (download)
/// link only when an owned file is present on disk — a missing issue still
/// appears, just without a download link.
fn issue_entry(base: &str, issue: &OpdsIssue) -> Entry {
    let title = match &issue.title {
        Some(t) if !t.is_empty() => format!("Issue #{}: {t}", issue.number),
        _ => format!("Issue #{}", issue.number),
    };

    let mut links = Vec::new();
    if issue.cover_url.as_deref().is_some_and(|u| !u.is_empty()) {
        links.push(Link::new(
            IMAGE_REL,
            abs(base, &format!("/opds/v1/covers/{}", issue.id)),
            "image/jpeg",
        ));
    }
    if let Some(filename) = &issue.present_filename {
        links.push(Link::new(
            ACQUISITION_REL,
            abs(base, &format!("/opds/v1/issues/{}/download", issue.id)),
            longbox_opds::archive_mime(filename),
        ));
    }

    Entry {
        id: format!("urn:longbox:issue:{}", issue.id),
        title,
        updated: rfc3339(issue.updated_at),
        dc_issued: issue.cover_date.clone().filter(|d| !d.is_empty()),
        summary: issue.summary.clone().filter(|s| !s.is_empty()),
        content: None,
        links,
    }
}

/// Pagination links for the search feed, which must carry `q` alongside
/// `page` in every href.
fn search_links(base: &str, q_encoded: &str, page: Page) -> Vec<Link> {
    let href = |n: u64| format!("{base}/opds/v1/search?q={q_encoded}&page={n}");
    let mut links = vec![
        Link::new("self", href(page.number), NAVIGATION_TYPE),
        start_link(base),
        search_descriptor_link(base),
        Link::new("up", abs(base, "/opds/v1"), NAVIGATION_TYPE),
    ];
    if page.has_prev() {
        links.push(Link::new(
            "previous",
            href(page.number - 1),
            NAVIGATION_TYPE,
        ));
    }
    if page.has_next() {
        links.push(Link::new("next", href(page.number + 1), NAVIGATION_TYPE));
    }
    links
}

/// `self` + `start` + `up` + conditional `next`/`previous` links for a
/// paginated feed.
fn paginated_links(base: &str, path: &str, page: Page, up_path: &str, kind: FeedKind) -> Vec<Link> {
    let mut links = vec![
        Link::new("self", page_href(base, path, page.number), kind.mime()),
        start_link(base),
        search_descriptor_link(base),
        Link::new("up", abs(base, up_path), NAVIGATION_TYPE),
    ];
    if page.has_prev() {
        links.push(Link::new(
            "previous",
            page_href(base, path, page.number - 1),
            kind.mime(),
        ));
    }
    if page.has_next() {
        links.push(Link::new(
            "next",
            page_href(base, path, page.number + 1),
            kind.mime(),
        ));
    }
    links
}

fn page_meta(page: Page) -> PageMeta {
    PageMeta {
        total_results: page.total,
        items_per_page: page.per_page,
        start_index: page.start_index(),
    }
}

fn self_link(base: &str, path: &str, kind: FeedKind) -> Link {
    Link::new("self", abs(base, path), kind.mime())
}

fn start_link(base: &str) -> Link {
    Link::new("start", abs(base, "/opds/v1"), NAVIGATION_TYPE)
}

/// `rel="search"` link to the OpenSearch descriptor, making search
/// discoverable from any feed.
fn search_descriptor_link(base: &str) -> Link {
    Link::new(
        "search",
        abs(base, "/opds/v1/opensearch.xml"),
        longbox_opds::opensearch::OPENSEARCH_DESCRIPTION_TYPE,
    )
}

// ---------- response + formatting primitives ----------

/// Render a navigation feed with the OPDS navigation content-type.
fn navigation_response(feed: Feed) -> Response {
    (
        [(header::CONTENT_TYPE, FeedKind::Navigation.mime())],
        feed.to_xml(),
    )
        .into_response()
}

/// Render an acquisition feed with the OPDS acquisition content-type.
fn acquisition_response(feed: Feed) -> Response {
    (
        [(header::CONTENT_TYPE, FeedKind::Acquisition.mime())],
        feed.to_xml(),
    )
        .into_response()
}

/// `{base}{path}` — `base` is trailing-slash-stripped, `path` starts `/`.
fn abs(base: &str, path: &str) -> String {
    format!("{base}{path}")
}

/// Absolute href carrying `?page=N`.
fn page_href(base: &str, path: &str, page: u64) -> String {
    format!("{base}{path}?page={page}")
}

/// Format a stored (UTC) timestamp as RFC 3339 for `<updated>`.
fn rfc3339(dt: time::PrimitiveDateTime) -> String {
    dt.assume_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// "1 series" / "3 series" style label (comic "series" is its own plural).
fn plural_count(n: i64, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}
