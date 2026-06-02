//! Newznab search request construction.

use crate::error::IndexerError;
use crate::types::IndexerConfig;

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
    let issue_part = if padded {
        match issue.parse::<u32>() {
            Ok(n) => format!("{n:03}"),
            Err(_) => issue.to_string(),
        }
    } else {
        issue.to_string()
    };
    format!("{series} {issue_part}")
}

/// Build the full Newznab search URL for an indexer + search term.
/// `t=search`, `cat=7030`, `o=xml`, `maxage` from the indexer config.
pub fn build_url(indexer: &IndexerConfig, search_term: &str) -> Result<String, IndexerError> {
    let base = indexer.base_url.trim_end_matches('/');
    let endpoint = format!("{base}/api");
    let maxage = indexer.maxage_days.to_string();
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
        assert_eq!(build_search_term("Wolverine", "5", true), "Wolverine 005");
        assert_eq!(build_search_term("Saga", "12", true), "Saga 012");
        assert_eq!(build_search_term("X", "100", true), "X 100");
    }

    #[test]
    fn unpadded_leaves_issue_as_is() {
        assert_eq!(build_search_term("Wolverine", "5", false), "Wolverine 5");
        assert_eq!(build_search_term("Saga", "12", false), "Saga 12");
    }

    #[test]
    fn non_numeric_issues_pass_through_both_variations() {
        assert_eq!(
            build_search_term("Bone", "Annual 1", true),
            "Bone Annual 1"
        );
        assert_eq!(
            build_search_term("Bone", "Annual 1", false),
            "Bone Annual 1"
        );
        assert_eq!(build_search_term("Promethea", "½", true), "Promethea ½");
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
            "Beneath the Trees Where Nobody Sees 005"
        );
        assert_eq!(
            build_search_term("Beneath the Trees Where Nobody Sees", "5", false),
            "Beneath the Trees Where Nobody Sees 5"
        );
    }

    #[test]
    fn url_carries_all_required_params() {
        let url = build_url(&cfg(), "Wolverine 005").unwrap();
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
        let url = build_url(&c, "x").unwrap();
        assert!(url.starts_with("https://idx.example.com/api?"));
        assert!(!url.contains("//api"));
    }
}
