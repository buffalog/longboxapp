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
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Title variants for the relaxation ladder, most-specific first:
///
/// 1. the full title (as given)
/// 2. the substring AFTER the first colon
/// 3. the substring BEFORE the first colon
///
/// Colon-less titles yield just `[full]`. Variants are returned RAW (not
/// query-normalized) so callers can reuse them as match-side aliases if
/// desired; `search_ladder` / `build_url` normalize at send time.
/// Empty/whitespace splits are dropped.
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

/// Hard cap on ladder length. The ladder is 3–4 rungs in practice
/// (precision, bare title, after-colon, aliases); the cap exists so a
/// pathological alias list cannot turn one issue into an unbounded
/// request burst.
pub const MAX_LADDER_RUNGS: usize = 6;

/// The ordered list of `q` search terms to try for an issue,
/// most-specific first. The caller stops at the first rung that yields a
/// candidate surviving `filter_by_series_title` — NOT the first rung
/// that returns HTTP results. See [`crate::client`] for why.
///
/// **The issue number cannot be only in the query, and cannot be only
/// out of it.** Newznab `t=search&q=` is a crude match over release
/// titles, and release titles are dot-separated scene names, so
/// appending the issue costs 75–95% of results and often reaches zero
/// (measured on DogNZB, window wide enough to see the back catalogue):
///
/// ```text
/// series                     bare   +001    +1
/// saga                        100     24     25
/// the department of truth      23      1      0
/// ice cream man                48      3      0
/// criminal                    100     12      3
/// 100 bullets brother lono     23      0      0
/// ```
///
/// Dropping it outright looks obviously right from that table and is
/// wrong — the table counts RESULTS, not whether the WANTED result is
/// among them. See the rung-order section below for the measurement
/// that settles it. The ladder therefore carries both forms.
///
/// The old padded/unpadded PAIR is gone — both differed only by the
/// issue token's shape, and unpadded measured worse than padded almost
/// everywhere. One padded precision rung replaces them.
///
/// A before-colon variant DOES ask the indexer about a different book
/// (`100 Bullets` for `100 Bullets: Brother Lono`), and an earlier
/// revision skipped it when that name was itself catalogued. Removed:
/// measured, a parent-series release scores 0.50 against
/// `100 Bullets: Brother Lono` and 0.20 against
/// `FBP: Federal Bureau of Physics`, both far below the 0.75 gate — so
/// the survival rule already stops that rung short-circuiting the
/// ladder AND already stops the grab. The skip bought one HTTP request
/// per colon-titled issue and cost a full-table `series_repo::find_all`
/// per issue attempt to find out. Not worth it.
///
/// # Rung order is precision-first, and rung 1 is NOT redundant
///
/// Rung 0 carries the issue number; rung 1 is the same series without
/// it. That looks duplicative and is not. **The narrow rung exists to
/// defeat server-side truncation on high-volume series.** Every
/// indexer caps a response (100 here), and the cap is applied before we
/// see anything, so on a series with more releases than the cap the
/// wanted issue can simply never arrive.
///
/// Measured on `Criminal` #1 across all eight configured indexers:
///
/// ```text
/// bare  "criminal"      -> 100, 100, 88, 100, 72, 52, 61, 7 results
///                          NOT ONE of them contains a Criminal 001
/// narrow "criminal 001"  -> 8 results on NZBSU, one of which IS
///                          "Criminal 001 (2019) (Digital) (Zone-Empire)"
/// ```
///
/// Three indexers hit the cap on the bare query. Deleting rung 0 as
/// "redundant" costs that issue entirely, and the failure is invisible —
/// it reads as "the release does not exist".
///
/// The reverse is also true, which is why rung 1 must stay: the narrow
/// query is a crude substring match against dot-separated scene names,
/// so `100 bullets brother lono 001` matches nothing at all while the
/// bare term returns all 8 issues. Neither rung subsumes the other.
///
/// Ordering them precision-first is only SAFE because of the success
/// rule in [`crate::client`]: a narrow rung that returns wrong-series
/// junk no longer stops the ladder, so its sole remaining effect is the
/// upside above. Under the old "any HTTP results wins" rule this
/// ordering would have been actively harmful.
pub fn search_ladder(series: &str, issue: &str, aliases: &[String]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut push = |t: String| {
        if !t.trim().is_empty() && !terms.contains(&t) && terms.len() < MAX_LADDER_RUNGS {
            terms.push(t);
        }
    };
    // 0: precision — narrows server-side past the result cap.
    push(precision_term(series, issue));
    // 1: the bare full title — the recall rung.
    let variants = title_variants(series);
    if let Some(full) = variants.first() {
        push(normalize_query(full));
    }
    // 2: after-colon. `Brother Lono` finds what `100 Bullets: Brother
    // Lono` misses when the scene name drops the prefix.
    if let Some(after) = variants.get(1) {
        push(normalize_query(after));
    }
    // 3: ALIASES BEFORE before-colon. The alias rung is the one that
    // carries a real recall win (it is what finds FBP under
    // `Collider`); the before-colon rung asks about a different book and
    // measured 0.50/0.20 against a 0.75 gate. When the cap bites, the
    // valuable rung must be the one that survives.
    for alias in aliases {
        push(normalize_query(alias));
    }
    if let Some(before) = variants.get(2) {
        push(normalize_query(before));
    }
    if terms.len() == MAX_LADDER_RUNGS {
        // Silent truncation of the ladder would reproduce, one layer up,
        // exactly the invisible failure this change exists to remove.
        tracing::debug!(
            target: "longbox_newznab",
            series = %series,
            cap = MAX_LADDER_RUNGS,
            "pull.ladder_capped"
        );
    }
    terms
}

/// `{series} {issue}` with a three-digit zero-pad on purely-numeric
/// issue numbers (`Wolverine 005`). Non-numeric ones (`Annual 1`, `½`)
/// pass through query-normalized. Padded only: measured across seven
/// series, the unpadded form returned fewer results than padded almost
/// everywhere, so it earned no rung of its own.
fn precision_term(series: &str, issue: &str) -> String {
    let series_q = normalize_query(series);
    let issue_q = match issue.parse::<u32>() {
        Ok(n) => format!("{n:03}"),
        Err(_) => normalize_query(issue),
    };
    format!("{series_q} {issue_q}").trim_end().to_string()
}

/// One-year slack added to an issue's age when computing the dynamic
/// maxage floor. Covers (a) digital releases posted ahead of the
/// printed cover date, and (b) a generous indexer clock-skew buffer.
const ISSUE_AGE_SLACK_DAYS: i64 = 365;

/// Newznab category id for comics (under the "Other" 7000 range).
const COMICS_CATEGORY: &str = "7030";

/// Result cap per indexer query — generous for one issue's releases.
const RESULT_LIMIT: u32 = 100;

/// Compute the effective `maxage` (in days) for a search against an
/// indexer for a particular issue. The static per-indexer
/// `configured_maxage` is treated as a FLOOR for fresh issues —
/// back-catalog issues automatically extend the window so old NZB
/// postings aren't filtered out at the indexer.
///
/// `cover_date` is the catalog issue's `YYYY-MM-DD` cover-date
/// string.
///
/// Formula: `max(configured, days_since_cover + ISSUE_AGE_SLACK_DAYS)`
/// where `days_since_cover` is the integer days between today and the
/// issue's cover date. A 3-month-old issue stays at the configured
/// value (typically 1500 days ≈ 4 years); a 20-year-old back-catalog
/// issue automatically opens to ~7660 days. Negative ages (cover_date
/// in the future, e.g. upcoming solicitations) clamp to the
/// configured floor.
///
/// **No usable date means [`MaxAge::Unlimited`], NOT the configured
/// floor.** Falling back to the floor treats absence of a date as
/// evidence the issue is recent, which is the same absence-as-evidence
/// error removed from the digest guards and the volume abstain rule.
/// The direction of error decides it: under-fetching produces a false
/// "does not exist", over-fetching produces more local parsing, which
/// is cheap and already happens on every result.
pub fn effective_maxage(configured_maxage: i64, cover_date: Option<&str>) -> MaxAge {
    let Some(cover) = cover_date.and_then(parse_yyyy_mm_dd) else {
        return MaxAge::Unlimited;
    };
    let today = time::OffsetDateTime::now_utc().date();
    let days_old = (today - cover).whole_days();
    if days_old <= 0 {
        return MaxAge::Days(configured_maxage);
    }
    MaxAge::Days(configured_maxage.max(days_old + ISSUE_AGE_SLACK_DAYS))
}

/// The `maxage` window for one search.
///
/// `Unlimited` omits the parameter entirely rather than sending a large
/// number. Measured against all eight configured indexers on a
/// back-catalogue query: omitting `maxage` returns byte-identical counts
/// to `maxage=100000` on every one of them (23/23, 63/63, 51/51, 18/18,
/// 31/31, 9/9, 23/23, 41/41), so no indexer treats an absent `maxage` as
/// a small default. Omission is the honest expression of "no window".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxAge {
    Days(i64),
    Unlimited,
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
    maxage: MaxAge,
) -> Result<String, IndexerError> {
    let base = indexer.base_url.trim_end_matches('/');
    let endpoint = format!("{base}/api");
    let limit = RESULT_LIMIT.to_string();
    let mut params: Vec<(&str, &str)> = vec![
        ("t", "search"),
        ("apikey", indexer.api_key.as_str()),
        ("q", search_term),
        ("cat", COMICS_CATEGORY),
        ("limit", limit.as_str()),
        ("o", "xml"),
    ];
    // `Unlimited` omits the parameter rather than sending a big number —
    // see `MaxAge`.
    let maxage_s;
    if let MaxAge::Days(d) = maxage {
        maxage_s = d.max(0).to_string();
        params.push(("maxage", maxage_s.as_str()));
    }
    let url = reqwest::Url::parse_with_params(&endpoint, params)
        .map_err(|e| IndexerError::HttpFailure(format!("bad indexer URL {endpoint:?}: {e}")))?;
    Ok(url.to_string())
}

/// The per-request result cap, exposed so callers can detect a rung that
/// came back exactly full — the signal that the indexer truncated and
/// the wanted release may be in the tail we never saw.
pub const fn result_limit() -> usize {
    RESULT_LIMIT as usize
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

    /// The precision rung's NON-NUMERIC branch. Ported back after the
    /// review found it uncovered: breaking `Err(_) => normalize_query`
    /// to `String::new()` passed the ENTIRE workspace — 75 targets,
    /// 1330 tests, zero failures. `build_search_term`'s deleted
    /// `non_numeric_issues_pass_through_both_variations` was the only
    /// thing covering it and nothing was ported when it went.
    ///
    /// Without this, `Bone` `Annual 1` would silently query `q="bone"`
    /// and no test would notice.
    #[test]
    fn the_precision_rung_keeps_non_numeric_issue_numbers() {
        let ladder = search_ladder("Bone", "Annual 1", &[]);
        assert_eq!(ladder.first().unwrap(), "bone annual 1");

        // `½` is `No` in Unicode and therefore alphanumeric to Rust, so
        // it survives normalisation rather than vanishing.
        let ladder = search_ladder("Promethea", "\u{00BD}", &[]);
        assert_eq!(ladder.first().unwrap(), "promethea \u{00BD}");
    }

    /// The YEAR must never be APPENDED to a query term. Named tripwire
    /// for the "ongoing series unfindable" bug: releases carry the
    /// RELEASE year, not the series `start_year`, so a literal
    /// `start_year` in `q` returns nothing for any ongoing series whose
    /// years differ. Restored after the review noted the original went
    /// out with `build_search_term`.
    ///
    /// A year that is genuinely PART of the title (`2000 AD`) rides
    /// along untouched — that is the series' name, not an injected
    /// filter — which is why this asserts on a year-free title.
    #[test]
    fn year_is_never_embedded_in_the_query_term() {
        let ladder = search_ladder("Wolverine", "5", &[]);
        assert_eq!(ladder, vec!["wolverine 005", "wolverine"]);
        for rung in &ladder {
            assert!(
                !rung
                    .chars()
                    .collect::<Vec<_>>()
                    .windows(4)
                    .any(|w| { w.iter().all(char::is_ascii_digit) && w[0] != '0' }),
                "no rung may carry a 4-digit year: {ladder:?}"
            );
        }
    }

    #[test]
    fn search_ladder_is_precision_first_then_recall() {
        let ladder = search_ladder(
            "FBP: Federal Bureau of Physics",
            "1",
            &["Collider".to_string()],
        );
        assert_eq!(
            ladder,
            vec![
                "fbp federal bureau of physics 001",
                "fbp federal bureau of physics",
                "federal bureau of physics",
                "collider",
                "fbp",
            ],
            "precision, bare, after-colon, ALIASES, then before-colon last"
        );
    }

    /// Rung 0 and rung 1 differ only by the issue token and NEITHER
    /// subsumes the other — rung 0 defeats server-side truncation on
    /// high-volume series, rung 1 defeats the crude substring match on
    /// scene names. A cleanup that deletes either as "redundant"
    /// breaks a measured case; this test is the tripwire.
    #[test]
    fn the_ladder_carries_both_the_narrow_and_the_bare_form() {
        let ladder = search_ladder("Criminal", "1", &[]);
        assert!(
            ladder.contains(&"criminal 001".to_string()),
            "narrow rung missing: {ladder:?}"
        );
        assert!(
            ladder.contains(&"criminal".to_string()),
            "bare rung missing: {ladder:?}"
        );
        assert!(
            ladder.iter().position(|t| t == "criminal 001")
                < ladder.iter().position(|t| t == "criminal"),
            "precision must come first: {ladder:?}"
        );
    }

    /// The padded/unpadded PAIR is gone — one padded precision rung
    /// replaces both — and a duplicate alias must not add a rung.
    #[test]
    fn search_ladder_has_no_duplicate_rungs() {
        let ladder = search_ladder("Saga", "1", &["Saga".to_string()]);
        assert_eq!(
            ladder,
            vec!["saga 001", "saga"],
            "alias identical to title collapses; no unpadded twin"
        );
    }

    /// When the cap bites, the ALIAS rung must survive and the
    /// before-colon rung must be the one dropped. The alias is what
    /// finds a series the scene names differently; the before-colon
    /// half asks about a different book entirely (measured 0.50 and
    /// 0.20 against a 0.75 gate, so it can never produce a grab).
    #[test]
    fn the_cap_drops_the_before_colon_rung_before_any_alias() {
        let aliases: Vec<String> = (0..5).map(|i| format!("alias {i}")).collect();
        let ladder = search_ladder("A: B", "1", &aliases);
        assert_eq!(ladder.len(), MAX_LADDER_RUNGS);
        assert!(
            !ladder.contains(&"a".to_string()),
            "before-colon must be sacrificed first: {ladder:?}"
        );
        assert!(
            ladder.contains(&"alias 0".to_string()) && ladder.contains(&"alias 2".to_string()),
            "aliases must survive the cap: {ladder:?}"
        );
    }

    #[test]
    fn search_ladder_is_capped() {
        let aliases: Vec<String> = (0..50).map(|i| format!("alias {i}")).collect();
        let ladder = search_ladder("A: B", "1", &aliases);
        assert_eq!(
            ladder.len(),
            6,
            "pinned to the actual shape, not to MAX_LADDER_RUNGS itself — \
             asserting against the constant passes any mutation of it"
        );
    }

    #[test]
    fn url_carries_all_required_params() {
        let url = build_url(&cfg(), "Wolverine 005", MaxAge::Days(1500)).unwrap();
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
        let url = build_url(&c, "x", MaxAge::Days(1500)).unwrap();
        assert!(url.starts_with("https://idx.example.com/api?"));
        assert!(!url.contains("//api"));
    }

    #[test]
    fn url_maxage_override_takes_precedence_over_indexer_config() {
        // The engine passes the issue-age-derived effective maxage so a
        // back-catalog issue doesn't get filtered out by the static
        // indexer config. The override path is what makes this work.
        let url = build_url(&cfg(), "Y The Last Man 029", MaxAge::Days(8400)).unwrap();
        assert!(url.contains("maxage=8400"));
        assert!(!url.contains("maxage=1500"));
    }

    // -------- effective_maxage --------

    /// No date is NOT evidence the issue is recent. Falling back to the
    /// configured floor under-fetches and produces a false "does not
    /// exist"; the widest window over-fetches and costs only local
    /// parsing. 35 of 403 missing issues in the live catalog have no
    /// usable cover_date.
    #[test]
    fn effective_maxage_is_unlimited_without_a_usable_date() {
        assert_eq!(effective_maxage(1500, None), MaxAge::Unlimited);
        for bad in ["", "garbage", "20240101", "2024-13-01", "2024-02-30"] {
            assert_eq!(
                effective_maxage(1500, Some(bad)),
                MaxAge::Unlimited,
                "malformed {bad:?} must not silently become the floor"
            );
        }
    }

    /// `Unlimited` omits the parameter rather than sending a big number.
    #[test]
    fn unlimited_omits_the_maxage_parameter() {
        let url = build_url(&cfg(), "saga", MaxAge::Unlimited).unwrap();
        assert!(!url.contains("maxage"), "{url}");
        let url = build_url(&cfg(), "saga", MaxAge::Days(1500)).unwrap();
        assert!(url.contains("maxage=1500"), "{url}");
    }

    #[test]
    fn effective_maxage_clamps_to_configured_for_future_cover_date() {
        // Solicitation issues with a cover_date months in the future
        // shouldn't shrink the window below the configured value.
        let next_year = (time::OffsetDateTime::now_utc().date().year() + 1).to_string();
        let date = format!("{next_year}-01-01");
        assert_eq!(effective_maxage(1500, Some(&date)), MaxAge::Days(1500));
    }

    #[test]
    fn effective_maxage_extends_for_back_catalog_issues() {
        // A 20-year-old comic should get a wide-open window, not the
        // configured 4-year (1500-day) default.
        let twenty_years_ago = (time::OffsetDateTime::now_utc().date().year() - 20).to_string();
        let date = format!("{twenty_years_ago}-01-01");
        let MaxAge::Days(n) = effective_maxage(1500, Some(&date)) else {
            panic!("a dated back-catalog issue must yield a bounded window");
        };
        // 20 years ≈ 7300 days + 365 slack = ~7665.
        assert!(
            (7300 + 365 - 1..=7300 + 365 + 366).contains(&n),
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
        assert_eq!(
            effective_maxage(1500, Some(&six_months)),
            MaxAge::Days(1500)
        );
    }
}
