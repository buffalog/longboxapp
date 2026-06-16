//! OPDS 1.2 Atom feed model + serialization.
//!
//! Pure, allocation-only construction of OPDS catalog feeds: the web layer
//! reads the database, builds a [`Feed`] of [`Entry`]s with absolute
//! hrefs, and calls [`Feed::to_xml`]. No web or DB types leak in here, so
//! the XML shape (and its escaping) is unit-testable in isolation.

use std::fmt::Write as _;

/// `type` for navigation feeds and links pointing at them.
pub const NAVIGATION_TYPE: &str = "application/atom+xml;profile=opds-catalog;kind=navigation";
/// `type` for acquisition feeds and links pointing at them.
pub const ACQUISITION_TYPE: &str = "application/atom+xml;profile=opds-catalog;kind=acquisition";
/// Items per page across every paginated list feed (spec-mandated).
pub const ITEMS_PER_PAGE: u64 = 25;

/// OPDS acquisition MIME type for a comic archive, by filename extension.
/// `.cbz`/`.cbr`/`.cb7` map to their `application/x-cb*` types; anything
/// else falls back to `application/octet-stream`. Shared by the acquisition
/// feed's download link and the streaming endpoint so the advertised type
/// and the served type always agree.
pub fn archive_mime(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".cbz") {
        "application/x-cbz"
    } else if lower.ends_with(".cbr") {
        "application/x-cbr"
    } else if lower.ends_with(".cb7") {
        "application/x-cb7"
    } else {
        "application/octet-stream"
    }
}

/// Pagination arithmetic for a single list feed. `number` is the 1-based
/// page; `new` clamps it (and `per_page`) to at least 1 so a `?page=0` or
/// `?page=-1` from a client can never produce a negative SQL offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub number: u64,
    pub per_page: u64,
    pub total: u64,
}

impl Page {
    pub fn new(number: u64, per_page: u64, total: u64) -> Self {
        Self {
            number: number.max(1),
            per_page: per_page.max(1),
            total,
        }
    }

    /// SQL `OFFSET` for this page.
    pub fn offset(&self) -> u64 {
        (self.number - 1) * self.per_page
    }

    /// SQL `LIMIT` for this page.
    pub fn limit(&self) -> u64 {
        self.per_page
    }

    /// OpenSearch `startIndex`: 1-based index of this page's first item.
    pub fn start_index(&self) -> u64 {
        self.offset() + 1
    }

    /// Total number of pages (0 when there are no results).
    pub fn total_pages(&self) -> u64 {
        if self.total == 0 {
            0
        } else {
            self.total.div_ceil(self.per_page)
        }
    }

    /// Whether a `rel="previous"` link applies.
    pub fn has_prev(&self) -> bool {
        self.number > 1
    }

    /// Whether a `rel="next"` link applies — i.e. items remain beyond this
    /// page.
    pub fn has_next(&self) -> bool {
        self.offset() + self.per_page < self.total
    }
}

/// Whether a feed (or a link's target) is navigation or acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    Navigation,
    Acquisition,
}

impl FeedKind {
    pub fn mime(self) -> &'static str {
        match self {
            FeedKind::Navigation => NAVIGATION_TYPE,
            FeedKind::Acquisition => ACQUISITION_TYPE,
        }
    }
}

/// An Atom `<link>`.
#[derive(Debug, Clone)]
pub struct Link {
    pub rel: String,
    pub href: String,
    pub mime: String,
}

impl Link {
    pub fn new(rel: impl Into<String>, href: impl Into<String>, mime: impl Into<String>) -> Self {
        Self {
            rel: rel.into(),
            href: href.into(),
            mime: mime.into(),
        }
    }
}

/// An Atom `<entry>`.
///
/// Navigation entries carry a `subsection` link to a sub-feed plus an
/// optional descriptive `content`. Acquisition entries (per issue) instead
/// carry image + acquisition links, a `dc_issued` cover date, and a
/// `summary`. All the descriptive fields are optional, so one struct serves
/// both feed kinds; a field left `None` is simply not emitted.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub updated: String,
    /// `<dc:issued>` — issue cover date (acquisition entries).
    pub dc_issued: Option<String>,
    /// `<summary>` — issue summary (acquisition entries).
    pub summary: Option<String>,
    /// `<content type="text">` — descriptive blurb (navigation entries).
    pub content: Option<String>,
    pub links: Vec<Link>,
}

/// OpenSearch pagination elements emitted at feed level.
#[derive(Debug, Clone, Copy)]
pub struct PageMeta {
    pub total_results: u64,
    pub items_per_page: u64,
    pub start_index: u64,
}

/// A complete OPDS catalog feed.
#[derive(Debug, Clone)]
pub struct Feed {
    pub id: String,
    pub title: String,
    pub updated: String,
    pub kind: FeedKind,
    /// Feed-level links: `self`, `start`, optional `up`/`search`, and the
    /// `next`/`previous` pagination links.
    pub links: Vec<Link>,
    pub pagination: Option<PageMeta>,
    pub entries: Vec<Entry>,
}

impl Feed {
    /// Serialize to an OPDS Atom XML document. All text and attribute
    /// values are XML-escaped.
    pub fn to_xml(&self) -> String {
        let mut out = String::with_capacity(512 + self.entries.len() * 256);
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str(
            "<feed xmlns=\"http://www.w3.org/2005/Atom\" \
             xmlns:dc=\"http://purl.org/dc/terms/\" \
             xmlns:opds=\"http://opds-spec.org/2010/catalog\" \
             xmlns:opensearch=\"http://a9.com/-/spec/opensearch/1.1/\">\n",
        );
        let _ = writeln!(out, "  <id>{}</id>", esc(&self.id));
        let _ = writeln!(out, "  <title>{}</title>", esc(&self.title));
        let _ = writeln!(out, "  <updated>{}</updated>", esc(&self.updated));
        for link in &self.links {
            write_link(&mut out, "  ", link);
        }
        if let Some(meta) = &self.pagination {
            let _ = writeln!(
                out,
                "  <opensearch:totalResults>{}</opensearch:totalResults>",
                meta.total_results
            );
            let _ = writeln!(
                out,
                "  <opensearch:itemsPerPage>{}</opensearch:itemsPerPage>",
                meta.items_per_page
            );
            let _ = writeln!(
                out,
                "  <opensearch:startIndex>{}</opensearch:startIndex>",
                meta.start_index
            );
        }
        for entry in &self.entries {
            out.push_str("  <entry>\n");
            let _ = writeln!(out, "    <title>{}</title>", esc(&entry.title));
            let _ = writeln!(out, "    <id>{}</id>", esc(&entry.id));
            let _ = writeln!(out, "    <updated>{}</updated>", esc(&entry.updated));
            for link in &entry.links {
                write_link(&mut out, "    ", link);
            }
            if let Some(issued) = &entry.dc_issued {
                let _ = writeln!(out, "    <dc:issued>{}</dc:issued>", esc(issued));
            }
            if let Some(summary) = &entry.summary {
                let _ = writeln!(out, "    <summary>{}</summary>", esc(summary));
            }
            if let Some(content) = &entry.content {
                let _ = writeln!(out, "    <content type=\"text\">{}</content>", esc(content));
            }
            out.push_str("  </entry>\n");
        }
        out.push_str("</feed>\n");
        out
    }
}

fn write_link(out: &mut String, indent: &str, link: &Link) {
    let _ = writeln!(
        out,
        "{indent}<link rel=\"{}\" href=\"{}\" type=\"{}\"/>",
        esc(&link.rel),
        esc(&link.href),
        esc(&link.mime),
    );
}

/// XML-escape a string for use in element text or a double-quoted
/// attribute. Conservatively escapes the full set `& < > " '` so the same
/// function is safe in both positions.
pub(crate) fn esc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    #[test]
    fn page_math_basic() {
        let p = Page::new(1, 25, 60);
        assert_eq!(p.offset(), 0);
        assert_eq!(p.limit(), 25);
        assert_eq!(p.start_index(), 1);
        assert_eq!(p.total_pages(), 3);
        assert!(!p.has_prev());
        assert!(p.has_next());

        let p2 = Page::new(2, 25, 60);
        assert_eq!(p2.offset(), 25);
        assert_eq!(p2.start_index(), 26);
        assert!(p2.has_prev());
        assert!(p2.has_next());

        let p3 = Page::new(3, 25, 60);
        assert_eq!(p3.offset(), 50);
        assert!(p3.has_prev());
        assert!(!p3.has_next()); // 50 + 25 = 75 >= 60
    }

    #[test]
    fn page_clamps_and_handles_empty() {
        // page 0 / negative-ish clamps to 1.
        let p = Page::new(0, 25, 0);
        assert_eq!(p.number, 1);
        assert_eq!(p.offset(), 0);
        assert_eq!(p.start_index(), 1);
        assert_eq!(p.total_pages(), 0);
        assert!(!p.has_prev());
        assert!(!p.has_next());
    }

    #[test]
    fn page_exact_multiple_has_no_extra_next() {
        let p = Page::new(2, 25, 50);
        assert_eq!(p.total_pages(), 2);
        assert!(!p.has_next());
    }

    fn sample_feed() -> Feed {
        Feed {
            id: "urn:longbox:opds:root".into(),
            title: "LongBox & Co".into(),
            updated: "2026-06-15T00:00:00Z".into(),
            kind: FeedKind::Navigation,
            links: vec![
                Link::new("self", "http://h/opds/v1/series?page=1", NAVIGATION_TYPE),
                Link::new("start", "http://h/opds/v1", NAVIGATION_TYPE),
                Link::new("next", "http://h/opds/v1/series?page=2", NAVIGATION_TYPE),
            ],
            pagination: Some(PageMeta {
                total_results: 60,
                items_per_page: 25,
                start_index: 1,
            }),
            entries: vec![Entry {
                id: "urn:longbox:series:1".into(),
                title: "Saga <Deluxe>".into(),
                updated: "2026-06-01T12:00:00Z".into(),
                content: Some("A \"space opera\" & more".into()),
                links: vec![Link::new(
                    "subsection",
                    "http://h/opds/v1/series/1",
                    ACQUISITION_TYPE,
                )],
                ..Default::default()
            }],
        }
    }

    #[test]
    fn to_xml_is_well_formed_and_escaped() {
        let xml = sample_feed().to_xml();

        // Parses cleanly end-to-end (proves well-formedness + that every
        // special char in titles/content was escaped).
        let mut reader = Reader::from_str(&xml);
        let mut depth = 0i32;
        let mut saw_feed = false;
        let mut saw_entry = false;
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    depth += 1;
                    match e.name().as_ref() {
                        b"feed" => saw_feed = true,
                        b"entry" => saw_entry = true,
                        _ => {}
                    }
                }
                Ok(Event::End(_)) => depth -= 1,
                Ok(Event::Eof) => break,
                Err(err) => panic!("malformed OPDS XML: {err}"),
                _ => {}
            }
        }
        assert_eq!(depth, 0, "unbalanced tags");
        assert!(saw_feed && saw_entry);

        // Raw escaping is present in the serialized bytes.
        assert!(xml.contains("<title>LongBox &amp; Co</title>"));
        assert!(xml.contains("Saga &lt;Deluxe&gt;"));
        assert!(xml.contains("A &quot;space opera&quot; &amp; more"));

        // OpenSearch pagination elements emitted.
        assert!(xml.contains("<opensearch:totalResults>60</opensearch:totalResults>"));
        assert!(xml.contains("<opensearch:itemsPerPage>25</opensearch:itemsPerPage>"));
        assert!(xml.contains("<opensearch:startIndex>1</opensearch:startIndex>"));

        // next link present.
        assert!(xml.contains(r#"rel="next""#));
    }

    #[test]
    fn acquisition_entry_emits_dc_issued_and_summary() {
        let feed = Feed {
            id: "urn:longbox:series:1".into(),
            title: "Saga".into(),
            updated: "2026-06-15T00:00:00Z".into(),
            kind: FeedKind::Acquisition,
            links: vec![Link::new("self", "http://h/opds/v1/series/1", ACQUISITION_TYPE)],
            pagination: Some(PageMeta {
                total_results: 1,
                items_per_page: 25,
                start_index: 1,
            }),
            entries: vec![Entry {
                id: "urn:longbox:issue:7".into(),
                title: "Issue #1".into(),
                updated: "2026-06-01T00:00:00Z".into(),
                dc_issued: Some("2024-01-15".into()),
                summary: Some("First & last".into()),
                content: None,
                links: vec![
                    Link::new("http://opds-spec.org/image", "http://h/c/7", "image/jpeg"),
                    Link::new(
                        "http://opds-spec.org/acquisition",
                        "http://h/d/7",
                        "application/x-cbz",
                    ),
                ],
            }],
        };
        let xml = feed.to_xml();
        assert!(xml.contains("<dc:issued>2024-01-15</dc:issued>"));
        assert!(xml.contains("<summary>First &amp; last</summary>"));
        assert!(xml.contains(r#"rel="http://opds-spec.org/acquisition""#));
        assert!(xml.contains(r#"type="application/x-cbz""#));
        // dc:issued must precede summary (spec entry ordering).
        let issued = xml.find("dc:issued").unwrap();
        let summary = xml.find("<summary>").unwrap();
        assert!(issued < summary);
    }

    #[test]
    fn archive_mime_maps_extensions() {
        assert_eq!(archive_mime("X 001.cbz"), "application/x-cbz");
        assert_eq!(archive_mime("X 001.CBR"), "application/x-cbr"); // case-insensitive
        assert_eq!(archive_mime("X 001.cb7"), "application/x-cb7");
        assert_eq!(archive_mime("X 001.pdf"), "application/octet-stream");
        assert_eq!(archive_mime("noext"), "application/octet-stream");
    }

    #[test]
    fn no_pagination_omits_opensearch_elements() {
        let mut feed = sample_feed();
        feed.pagination = None;
        let xml = feed.to_xml();
        assert!(!xml.contains("opensearch:totalResults"));
        assert!(!xml.contains("opensearch:startIndex"));
    }
}
