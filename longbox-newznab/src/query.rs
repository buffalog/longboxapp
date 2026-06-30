//! Newznab search request construction.

use crate::error::IndexerError;
use crate::types::IndexerConfig;

/// Aggressively normalize a title into a Newznab `q` term. Indexers
/// AND-tokenize `q` and Prowlarr forwards it verbatim (no CleanTitle), so
/// every punctuation char that a scene release name omits is a recall cut.
/// Lowercase, map ALL non-alphanumeric (colon, hyphen, dot, paren, slash,
/// ampersand…) to space, collapse whitespace. Hyphen→space here is the
/// query-side half of the hyphen agreement (`select::match_normalize` is the
/// match-side half).
pub fn normalize_query(s: &str) -> String {
    let lowered: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Title variants for the relaxation ladder, most-specific first:
/// 1. the full title (as given), 2. the substring AFTER the first colon,
/// 3. the substring BEFORE the first colon. Colon-less titles yield just
/// `[full]`. Variants are returned RAW (not query-normalized) so callers can
/// reuse them as match-side aliases if desired; `build_search_term` /
/// `build_url` normalize at send time. Empty/whitespace splits are dropped.
pub fn title_variants(title: &str) -> Vec<String> {
    let mut out = vec![title.to_string()];
    if let Some(idx) = title.find(':') {
        let after = title[idx + 1..].trim();
        let before = title[..idx].trim();
        if !after.is_empty() {
            out.push(after.to_string());
        }
        if !before.is_empty() {
            out.push(before.to_string());
        }
    }
    out
}

/// One-year slack added to an issue's age when computing the dynamic
/// maxage floor. Covers (a) digital releases posted ahead of the
/// printed cover date, and (b) a generous indexer clock-skew buffer.
const ISSUE_AGE_SLACK_DAYS: i64 = 365;

/// Newznab category id for comics (under the "Other" 7000 range).
const COMICS_CATEGORY: &str = "7030";

/// Result cap per indexer query — generous for one issue's releases.
const RESULT_LIMIT: u32 = 100;

/// Build the `q` search term for an issue.
///
/// `padded` selects the two-variation strategy from the brief:
/// `true` → three-digit zero-pad (`Wolverine 005`), `false` → as-is
/// (`Wolverine 5`). Padding applies only to purely-numeric issue
/// numbers; non-numeric ones (`Annual 1`, `½`) pass through unchanged
/// in both variations.
///
/// **No year in the query.** Newznab's `t=search` does a literal
/// substring match on `q`. Releases are tagged with their RELEASE
/// year, not the series's start_year, so a literal `(2024)` from the
/// catalog never appears in NZBs tagged `(2023)` and the query
/// returns zero results for any ongoing series whose start_year
/// differs from the issue's actual release year. Prowlarr — which
/// returns 85 hits for a query LongBox got 0 for — does not embed a
/// year in its text query; we follow the same shape.
///
/// Year is still load-bearing for ranking via the similarity filter
/// downstream (`filter_by_series_title`); it just doesn't belong in
/// the indexer text query.
pub fn build_search_term(series: &str, issue: &str, padded: bool) -> String {
    let series_q = normalize_query(series);
    let issue_part = if padded {
        match issue.parse::<u32>() {
            Ok(n) => format!("{n:03}"),
            Err(_) => normalize_query(issue),
        }
    } else {
        normalize_query(issue)
    };
    // ponytail: trim_end drops trailing space when issue_part is empty (e.g. "½" normalizes to "").
    format!("{series_q} {issue_part}").trim_end().to_string()
}

/// Compute the effective `maxage` (in days) for a search against an
/// indexer for a particular issue. The static per-indexer
/// `configured_maxage` is treated as a FLOOR for fresh issues —
/// back-catalog issues automatically extend the window so old NZB
/// postings aren't filtered out at the indexer.
///
/// `cover_date` is the catalog issue's `YYYY-MM-DD` cover-date
/// string. `None` (or a malformed value) falls back to the
/// configured floor — same behavior as before this helper existed.
///
/// Formula: `max(configured, days_since_cover + ISSUE_AGE_SLACK_DAYS)`
/// where `days_since_cover` is the integer days between today and the
/// issue's cover date. A 3-month-old issue stays at the configured
/// value (typically 1500 days ≈ 4 years); a 20-year-old back-catalog
/// issue automatically opens to ~7660 days. Negative ages (cover_date
/// in the future, e.g. upcoming solicitations) clamp to the
/// configured floor.
pub fn effective_maxage_days(configured_maxage: i64, cover_date: Option<&str>) -> i64 {
    let Some(date_str) = cover_date else {
        return configured_maxage;
    };
    let Some(cover) = parse_yyyy_mm_dd(date_str) else {
        return configured_maxage;
    };
    let today = time::OffsetDateTime::now_utc().date();
    let days_old = (today - cover).whole_days();
    if days_old <= 0 {
        return configured_maxage;
    }
    configured_maxage.max(days_old + ISSUE_AGE_SLACK_DAYS)
}

fn parse_yyyy_mm_dd(s: &str) -> Option<time::Date> {
    if s.len() < 10 {
        return None;
    }
    let year: i32 = s.get(..4)?.parse().ok()?;
    let month: u8 = s.get(5..7)?.parse().ok()?;
    let day: u8 = s.get(8..10)?.parse().ok()?;
    let month = time::Month::try_from(month).ok()?;
    time::Date::from_calendar_date(year, month, day).ok()
}

/// Build the full Newznab search URL for an indexer + search term.
/// `t=search`, `cat=7030`, `o=xml`. `maxage` comes from the indexer
/// config by default; callers pass `Some(N)` to override (the engine
/// does this per-issue via [`effective_maxage_days`] so back-catalog
/// searches aren't filtered server-side by stale postings).
pub fn build_url(
    indexer: &IndexerConfig,
    search_term: &str,
    maxage_override: Option<i64>,
) -> Result<String, IndexerError> {
    let base = indexer.base_url.trim_end_matches('/');
    let endpoint = format!("{base}/api");
    let maxage = maxage_override
        .unwrap_or(i64::from(indexer.maxage_days))
        .max(0)
        .to_string();
    let limit = RESULT_LIMIT.to_string();
    let params: [(&str, &str); 7] = [
        ("t", "search"),
        ("apikey", indexer.api_key.as_str()),
        ("q", search_term),
        ("cat", COMICS_CATEGORY),
        ("maxage", maxage.as_str()),
        ("limit", limit.as_str()),
        ("o", "xml"),
    ];
    let url = reqwest::Url::parse_with_params(&endpoint, params)
        .map_err(|e| IndexerError::HttpFailure(format!("bad indexer URL {endpoint:?}: {e}")))?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IndexerId;

    fn cfg() -> IndexerConfig {
        IndexerConfig {
            id: IndexerId(1),
            name: "test".into(),
            base_url: "https://idx.example.com".into(),
            api_key: "KEY123".into(),
            priority: 0,
            maxage_days: 1500,
        }
    }

    #[test]
    fn padded_zero_pads_numeric_issues() {
        assert_eq!(build_search_term("Wolverine", "5", true), "wolverine 005");
        assert_eq!(build_search_term("Saga", "12", true), "saga 012");
        assert_eq!(build_search_term("X", "100", true), "x 100");
    }

    #[test]
    fn unpadded_leaves_issue_as_is() {
        assert_eq!(build_search_term("Wolverine", "5", false), "wolverine 5");
        assert_eq!(build_search_term("Saga", "12", false), "saga 12");
    }

    #[test]
    fn non_numeric_issues_pass_through_both_variations() {
        assert_eq!(
            build_search_term("Bone", "Annual 1", true),
            "bone annual 1"
        );
        assert_eq!(
            build_search_term("Bone", "Annual 1", false),
            "bone annual 1"
        );
        // "½" is Unicode No (Number, other) — Rust's is_alphanumeric() returns true for it,
        // so it passes through normalize_query unchanged.
        assert_eq!(build_search_term("Promethea", "½", true), "promethea ½");
    }

    #[test]
    fn year_is_never_embedded_in_the_query_term() {
        // Regression: prior shape was `Series 005 (YYYY)`, which made
        // ongoing series unfindable. Newznab `t=search` does a literal
        // substring match on `q`, and NZB titles carry the RELEASE
        // year, not the series start_year — so a series that started
        // in 2023 with releases tagged "(2024)" never matched
        // "Series 005 (2023)". The signature no longer carries year;
        // this test is the load-bearing documentation that both
        // variations stay year-free.
        assert_eq!(
            build_search_term("Beneath the Trees Where Nobody Sees", "5", true),
            "beneath the trees where nobody sees 005"
        );
        assert_eq!(
            build_search_term("Beneath the Trees Where Nobody Sees", "5", false),
            "beneath the trees where nobody sees 5"
        );
    }

    #[test]
    fn search_term_is_query_normalized() {
        // Colon and mixed case in series title both collapse: "FBP: Federal…"
        // → "fbp federal bureau of physics". Numeric issue pads normally.
        assert_eq!(
            build_search_term("FBP: Federal Bureau of Physics", "5", true),
            "fbp federal bureau of physics 005"
        );
    }

    #[test]
    fn title_variants_yields_full_then_colon_splits() {
        assert_eq!(
            title_variants("FBP: Federal Bureau of Physics"),
            vec![
                "FBP: Federal Bureau of Physics",
                "Federal Bureau of Physics",
                "FBP"
            ]
        );
    }

    #[test]
    fn title_variants_no_colon_is_single() {
        assert_eq!(title_variants("Saga"), vec!["Saga"]);
    }

    #[test]
    fn url_carries_all_required_params() {
        let url = build_url(&cfg(), "Wolverine 005", None).unwrap();
        assert!(url.starts_with("https://idx.example.com/api?"));
        assert!(url.contains("t=search"));
        assert!(url.contains("apikey=KEY123"));
        assert!(url.contains("cat=7030"));
        assert!(url.contains("maxage=1500"));
        assert!(url.contains("limit=100"));
        assert!(url.contains("o=xml"));
        // Query term is percent-encoded.
        assert!(url.contains("q=Wolverine+005") || url.contains("q=Wolverine%20005"));
    }

    #[test]
    fn url_normalizes_trailing_slash_on_base() {
        let mut c = cfg();
        c.base_url = "https://idx.example.com/".into();
        let url = build_url(&c, "x", None).unwrap();
        assert!(url.starts_with("https://idx.example.com/api?"));
        assert!(!url.contains("//api"));
    }

    #[test]
    fn url_maxage_override_takes_precedence_over_indexer_config() {
        // The engine passes the issue-age-derived effective maxage so a
        // back-catalog issue doesn't get filtered out by the static
        // indexer config. The override path is what makes this work.
        let url = build_url(&cfg(), "Y The Last Man 029", Some(8400)).unwrap();
        assert!(url.contains("maxage=8400"));
        assert!(!url.contains("maxage=1500"));
    }

    // -------- effective_maxage_days --------

    #[test]
    fn effective_maxage_returns_configured_for_missing_cover_date() {
        assert_eq!(effective_maxage_days(1500, None), 1500);
    }

    #[test]
    fn effective_maxage_returns_configured_for_malformed_cover_date() {
        // Garbage strings, truncated, non-ISO formats — all fall back.
        assert_eq!(effective_maxage_days(1500, Some("")), 1500);
        assert_eq!(effective_maxage_days(1500, Some("garbage")), 1500);
        assert_eq!(effective_maxage_days(1500, Some("20240101")), 1500);
        assert_eq!(effective_maxage_days(1500, Some("2024-13-01")), 1500);
        assert_eq!(effective_maxage_days(1500, Some("2024-02-30")), 1500);
    }

    #[test]
    fn effective_maxage_clamps_to_configured_for_future_cover_date() {
        // Solicitation issues with a cover_date months in the future
        // shouldn't shrink the window below the configured value.
        let next_year = (time::OffsetDateTime::now_utc().date().year() + 1).to_string();
        let date = format!("{next_year}-01-01");
        assert_eq!(effective_maxage_days(1500, Some(&date)), 1500);
    }

    #[test]
    fn effective_maxage_extends_for_back_catalog_issues() {
        // A 20-year-old comic should get a wide-open window, not the
        // configured 4-year (1500-day) default.
        let twenty_years_ago = (time::OffsetDateTime::now_utc().date().year() - 20).to_string();
        let date = format!("{twenty_years_ago}-01-01");
        let n = effective_maxage_days(1500, Some(&date));
        // 20 years ≈ 7300 days + 365 slack = ~7665.
        assert!(
            n >= 7300 + 365 - 1 && n <= 7300 + 365 + 366,
            "expected ~7665, got {n}"
        );
    }

    #[test]
    fn effective_maxage_does_not_shrink_below_configured() {
        // A recent issue (cover 6 months ago) should keep the
        // configured 1500-day floor; the dynamic value would be ~545
        // and the max preserves the floor.
        let today = time::OffsetDateTime::now_utc().date();
        let six_months = today
            .checked_sub(time::Duration::days(180))
            .unwrap()
            .to_string();
        assert_eq!(effective_maxage_days(1500, Some(&six_months)), 1500);
    }
}
