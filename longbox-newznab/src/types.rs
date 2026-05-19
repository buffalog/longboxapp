use time::OffsetDateTime;

/// Identifies an indexer for error attribution. Maps to the
/// `indexer_configs` row id (Step 3); a newtype so the
/// `AllIndexersFailed` tuple is self-documenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IndexerId(pub i64);

/// One Newznab indexer's connection config. `longbox-newznab` owns
/// this input type rather than depending on `longbox-db`; Step 3's
/// `indexer_configs` row converts into it (disabled rows filtered out
/// by the caller before they reach this crate).
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub id: IndexerId,
    /// Human-readable name — used in logs + error attribution.
    pub name: String,
    /// Base URL of the indexer, e.g. `https://api.nzbgeek.info`.
    /// The `/api` path is appended by the client.
    pub base_url: String,
    pub api_key: String,
    /// Lower number = queried first. `find_release` sorts ascending.
    pub priority: i32,
    /// `maxage` query bound — only releases posted within this many
    /// days are returned.
    pub maxage_days: u32,
}

/// Archive format inferred from a release title. Newznab has no
/// structured format field; we sniff `.cbz` / `.cbr` from the title
/// string. Used only as a selection sort-key — both cbz and cbr are
/// acceptable results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Cbz,
    Cbr,
    Unknown,
}

/// One search result — a single NZB release for an issue.
///
/// Every Newznab-extension field is `Option`: indexers vary in which
/// `<newznab:attr>` tags they emit, and the defensive-parsing rule
/// (brief) is "missing fields handled gracefully, never panic."
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// Release name, e.g. `Wolverine 005 (1982) (digital).cbz`.
    pub title: String,
    /// NZB download URL — from the `<enclosure url="...">` element.
    pub nzb_url: String,
    /// RSS `<guid>` — stable per-release identifier. Used downstream
    /// (Step 6) as the `release_id` for retry-exclusion tracking.
    pub guid: String,
    /// `<pubDate>` parsed from RFC-2822. `None` when absent/unparseable.
    pub published: Option<OffsetDateTime>,
    /// `<newznab:attr name="size">` — bytes.
    pub size_bytes: Option<i64>,
    /// `<newznab:attr name="grabs">` — download count. Selection
    /// prefers higher.
    pub grabs: Option<i64>,
    /// `<newznab:attr name="category">` — numeric category id(s).
    pub category: Option<String>,
}
