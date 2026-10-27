//! `Series` domain type. The constructor computes `sort_title` via
//! [`normalize_title`] so that both sides of a matcher comparison live in the
//! same normalized space.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::normalize::normalize_title;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    /// ComicVine volume ID. Nullable; unique when set.
    pub cv_id: Option<i64>,
    /// Metron series slug. Reserved for Phase B; never written in Phase A.
    pub metron_id: Option<String>,
    pub title: String,
    /// Normalized form of `title` used for similarity matching. Always
    /// recomputable from `title` via [`normalize_title`].
    pub sort_title: String,
    pub start_year: Option<i32>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

impl Series {
    /// Construct a series with `sort_title` computed from `title`.
    pub fn new(
        id: i64,
        cv_id: Option<i64>,
        title: impl Into<String>,
        start_year: Option<i32>,
    ) -> Self {
        let title = title.into();
        let sort_title = normalize_title(&title);
        Self {
            id,
            cv_id,
            metron_id: None,
            title,
            sort_title,
            start_year,
            publisher: None,
            description: None,
            cover_url: None,
        }
    }
}

/// Why a ComicVine volume looks like a collected edition (TPB/HC) rather
/// than a series of single issues, or `None`.
///
/// Two signals, either one enough:
/// - The description's FIRST sentence names the format: "Trade paperbacks
///   collecting Green Arrow", "Hardcover collecting…", "Collects Criminal
///   #1-5". Ongoing series mention their trades too, but later ("…was
///   collected into ten trade paperbacks", "Collected in Absolute Batman"),
///   so only the opening sentence counts, and bare "collected" never does.
/// - At least half the named issues are "Volume N" / "Vol. N"
///   ("Volume 1: The Cobra Strikes!"). "Book One" is a prestige-format
///   issue name, not a trade marker.
///
/// Measured 2026-10-26 on the live catalog (926 series): 36 hits, every
/// one a trade, hardcover or original graphic novel. Original graphic
/// novels are single books, but they still need confirmation to add.
pub fn collected_edition_reason(description: Option<&str>, issue_names: &[&str]) -> Option<String> {
    static TAG: OnceLock<Regex> = OnceLock::new();
    static VOLUME: OnceLock<Regex> = OnceLock::new();
    let text = TAG
        .get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
        .replace_all(description.unwrap_or(""), " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = text.split(". ").next().unwrap_or("").to_lowercase();
    // "…are collected in the following trade paperbacks" opens an ONGOING
    // series' description (The Exterminators).
    let points_elsewhere = first.contains("collected in");
    if !points_elsewhere
        && ["trade paperback", "hardcover", "collecting", "collects"]
            .iter()
            .any(|k| first.contains(k))
    {
        return Some(format!(
            "ComicVine describes it as \"{}\"",
            text.split(". ").next().unwrap_or("").trim()
        ));
    }

    let volume = VOLUME.get_or_init(|| {
        Regex::new(
            r"(?i)\bvol(?:ume)?\.?\s*(?:\d+|one|two|three|four|five|six|seven|eight|nine|ten)\b",
        )
        .unwrap()
    });
    let named: Vec<&str> = issue_names
        .iter()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .collect();
    let volumes: Vec<&str> = named
        .iter()
        .copied()
        .filter(|n| volume.is_match(n))
        .collect();
    if !volumes.is_empty() && volumes.len() * 2 >= named.len() {
        return Some(format!(
            "{} of {} named issues are volumes, e.g. \"{}\"",
            volumes.len(),
            named.len(),
            volumes[0]
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four trade volumes found on the pull list on 2026-10-26, as CV
    /// lists them, and the lookalikes that must stay real series.
    #[test]
    fn collected_edition_reason_flags_trades_not_lookalikes() {
        let trades: [(&str, Option<&str>, &[&str]); 5] = [
            // Description alone: no named issues.
            ("Subgenre (2024)", Some("<p>Hardcover collecting <a href=\"/subgenre/4050-1/\">Subgenre</a>.</p>"), &[]),
            ("Hyde Street (2025)", Some("<p>Series of trade paperbacks based on <a href=\"/hyde-street/4050-160218/\">Hyde Street</a></p>"), &["Volume One", "Volume Two"]),
            ("G.I. Joe (2025)", Some(" "), &["Volume 1: The Cobra Strikes!", "Volume 2: Bludd's Revenge", "Volume 3: Dreadnok War", ""]),
            ("Invincible Universe: Battle Beast (2025)", Some("<p>Trade paperbacks collecting <a href=\"/x/4050-164334/\">Invincible Universe: Battle Beast</a>.</p>"), &["Volume One: Heart of Glory", ""]),
            ("Murder Podcast (2026)", None, &["Volume One"]),
        ];
        for (name, desc, issues) in trades {
            assert!(
                collected_edition_reason(desc, issues).is_some(),
                "{name} must be flagged"
            );
        }

        let real: [(&str, Option<&str>, &[&str]); 5] = [
            ("The Exterminators", Some("<p>The issues below are collected in the following trade paperbacks #1-5: Bug Brothers</p>"), &[]),
            ("The Nice House by the Sea", None, &["Book One", "Book Two", "Book Three", "Book Four"]),
            ("Absolute Batman", Some("<p>Collected in <a href=\"/absolute-batman/4050-167340/\">Absolute Batman</a>.</p>"), &["The Zoo, Part One", "The Zoo, Part Two"]),
            ("Y The Last Man", Some("<p>A tale about Yorick Brown, the last man on Earth. The entire series was collected into ten trade paperbacks.</p>"), &[]),
            ("Saga", Some("<p>Saga is an ongoing series.</p><h4>Collected Editions</h4><p>Trade paperbacks Volume 1 (#1-6)</p>"), &["Chapter One", "Chapter Two"]),
        ];
        for (name, desc, issues) in real {
            assert_eq!(
                collected_edition_reason(desc, issues),
                None,
                "{name} is a real series"
            );
        }
    }

    #[test]
    fn new_computes_sort_title() {
        let s = Series::new(1, Some(42), "The Walking Dead", Some(2003));
        assert_eq!(s.title, "The Walking Dead");
        assert_eq!(s.sort_title, "walking dead");
        assert_eq!(s.cv_id, Some(42));
        assert_eq!(s.start_year, Some(2003));
    }

    #[test]
    fn sort_title_equals_normalized_title() {
        let s = Series::new(1, None, "Spider-Man: Far From Home", None);
        assert_eq!(s.sort_title, normalize_title(&s.title));
    }

    #[test]
    fn metron_id_default_none() {
        let s = Series::new(1, None, "Saga", None);
        assert!(s.metron_id.is_none());
    }
}
