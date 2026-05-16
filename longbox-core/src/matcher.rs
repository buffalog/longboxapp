//! Three-tier match cascade.
//!
//! Tier 1 — `<Web>` URL extraction (confidence 1.0). CV beats Metron when
//! both URLs are present.
//!
//! Tier 2 — ComicInfo `<Series>` + `<Number>`. Series text and pool
//! `sort_title`s are compared in normalized space. Confidence = similarity
//! score in `[0.65, 1.0)`. Below 0.65, fall through to Tier 3.
//!
//! Tier 3 — filename parsing. Same similarity machinery as Tier 2, but
//! confidence is capped at [`crate::FILENAME_CONFIDENCE_CEILING`] (0.90) so
//! embedded metadata always wins a tie.
//!
//! No match in any tier → `MatchMethod::Unmatched`, confidence 0.0.
//!
//! The `threshold` parameter is accepted for API stability (it appears in the
//! Phase A spec's signature) but is not consulted by the cascade. Callers
//! pass the same threshold to [`crate::classify_status`] to derive the
//! `owned` / `needs_review` status from the returned confidence.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::comicinfo::ComicInfo;
use crate::file::MatchMethod;
use crate::filename::{parse as parse_filename, ParsingPattern};
use crate::issue::{Issue, IssueNumber};
use crate::normalize::normalize_title;
use crate::series::Series;
use crate::similarity::similarity;
use crate::{FILENAME_CONFIDENCE_CEILING, NEEDS_REVIEW_FLOOR};

#[derive(Debug, Clone, Copy)]
pub struct FileContext<'a> {
    /// Basename of the comic file (e.g. `"Saga 1 (2012).cbz"`). Scanner is
    /// responsible for stripping the directory.
    pub filename: &'a str,
    /// Parsed ComicInfo.xml if the .cbz had one. `None` for archives without
    /// ComicInfo or for `.cbr` / `.cb7` formats Phase A doesn't extract.
    pub comicinfo: Option<&'a ComicInfo>,
    /// User-editable filename patterns. Typically `parsing_patterns` rows.
    pub patterns: &'a [ParsingPattern],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchResult {
    pub issue_id: Option<i64>,
    pub method: MatchMethod,
    pub confidence: f64,
}

pub fn match_file(
    ctx: &FileContext<'_>,
    series_pool: &[Series],
    issue_pool: &[Issue],
    _threshold: f64,
) -> MatchResult {
    if let Some(ci) = ctx.comicinfo {
        if let Some(result) = tier1_web_url(ci, issue_pool) {
            return result;
        }
        if let Some(result) = tier2_comicinfo_text(ci, series_pool, issue_pool) {
            return result;
        }
    }

    if let Some(result) = tier3_filename(ctx, series_pool, issue_pool) {
        return result;
    }

    MatchResult {
        issue_id: None,
        method: MatchMethod::Unmatched,
        confidence: 0.0,
    }
}

fn tier1_web_url(ci: &ComicInfo, issue_pool: &[Issue]) -> Option<MatchResult> {
    if let Some(cv_id) = ci.cv_issue_id() {
        if let Some(issue) = issue_pool.iter().find(|i| i.cv_id == Some(cv_id)) {
            return Some(MatchResult {
                issue_id: Some(issue.id),
                method: MatchMethod::ComicInfoWebCv,
                confidence: 1.0,
            });
        }
    }
    if let Some(slug) = ci.metron_issue_slug() {
        if let Some(issue) = issue_pool
            .iter()
            .find(|i| i.metron_id.as_deref() == Some(slug.as_str()))
        {
            return Some(MatchResult {
                issue_id: Some(issue.id),
                method: MatchMethod::ComicInfoWebMetron,
                confidence: 1.0,
            });
        }
    }
    None
}

fn tier2_comicinfo_text(
    ci: &ComicInfo,
    series_pool: &[Series],
    issue_pool: &[Issue],
) -> Option<MatchResult> {
    let (series_text, number_text) = match (ci.series.as_deref(), ci.number.as_deref()) {
        (Some(s), Some(n)) => (s, n),
        _ => return None,
    };

    let (score, series) = best_series_match(series_text, ci.volume, series_pool)?;
    if score < NEEDS_REVIEW_FLOOR {
        return None;
    }

    let needle = IssueNumber::new(number_text);
    let issue = issue_pool
        .iter()
        .find(|i| i.series_id == series.id && i.number.matches(&needle))?;

    Some(MatchResult {
        issue_id: Some(issue.id),
        method: MatchMethod::ComicInfoXml,
        confidence: score,
    })
}

fn tier3_filename(
    ctx: &FileContext<'_>,
    series_pool: &[Series],
    issue_pool: &[Issue],
) -> Option<MatchResult> {
    let parsed = parse_filename(ctx.filename, ctx.patterns)?;
    let (score, series) = best_series_match(&parsed.series, parsed.year, series_pool)?;
    let confidence = score.min(FILENAME_CONFIDENCE_CEILING);
    if confidence < NEEDS_REVIEW_FLOOR {
        return None;
    }

    let needle = IssueNumber::new(&parsed.number);
    let issue = issue_pool
        .iter()
        .find(|i| i.series_id == series.id && i.number.matches(&needle))?;

    Some(MatchResult {
        issue_id: Some(issue.id),
        method: MatchMethod::FilenameRegex,
        confidence,
    })
}

/// Score every series in the pool against `candidate_text` (normalized), then
/// return the highest-scoring series. Tie-break: prefer series whose
/// `start_year` equals `candidate_year` (only when scores are bit-equal,
/// matching the user's confirmed rule). Final tie-break: lower `series.id`
/// (first-added wins) for determinism.
fn best_series_match<'s>(
    candidate_text: &str,
    candidate_year: Option<i32>,
    series_pool: &'s [Series],
) -> Option<(f64, &'s Series)> {
    if series_pool.is_empty() {
        return None;
    }
    let normalized = normalize_title(candidate_text);

    let mut scored: Vec<(f64, &Series)> = series_pool
        .iter()
        .map(|s| (similarity(&normalized, &s.sort_title), s))
        .collect();

    scored.sort_by(|a, b| {
        // Primary: score descending (bit-equal comparison per agreed spec).
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                // Volume / start_year tie-break only fires on exact score ties.
                let a_year_hit = candidate_year.is_some() && a.1.start_year == candidate_year;
                let b_year_hit = candidate_year.is_some() && b.1.start_year == candidate_year;
                b_year_hit.cmp(&a_year_hit)
            })
            .then_with(|| a.1.id.cmp(&b.1.id))
    });

    scored.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filename::default_patterns;
    use crate::DEFAULT_MATCH_THRESHOLD;

    fn series(id: i64, cv_id: Option<i64>, title: &str, year: Option<i32>) -> Series {
        Series::new(id, cv_id, title, year)
    }

    fn issue(id: i64, series_id: i64, cv_id: Option<i64>, number: &str) -> Issue {
        Issue {
            id,
            series_id,
            cv_id,
            metron_id: None,
            number: IssueNumber::new(number),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        }
    }

    fn issue_with_metron(id: i64, series_id: i64, metron: &str, number: &str) -> Issue {
        Issue {
            id,
            series_id,
            cv_id: None,
            metron_id: Some(metron.to_owned()),
            number: IssueNumber::new(number),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        }
    }

    fn ctx<'a>(
        filename: &'a str,
        ci: Option<&'a ComicInfo>,
        patterns: &'a [ParsingPattern],
    ) -> FileContext<'a> {
        FileContext {
            filename,
            comicinfo: ci,
            patterns,
        }
    }

    // -------- Tier 1 --------

    #[test]
    fn tier1_cv_web_url_hits() {
        let series = vec![series(1, Some(42), "The Walking Dead", Some(2003))];
        let issues = vec![issue(10, 1, Some(12345), "1")];
        let ci = ComicInfo {
            web: vec!["https://comicvine.gamespot.com/issue/4000-12345/".into()],
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("anything.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.issue_id, Some(10));
        assert_eq!(r.method, MatchMethod::ComicInfoWebCv);
        assert_eq!(r.confidence, 1.0);
    }

    #[test]
    fn tier1_metron_web_url_hits() {
        let series = vec![series(1, None, "Saga", Some(2012))];
        let issues = vec![issue_with_metron(10, 1, "saga-1-2012", "1")];
        let ci = ComicInfo {
            web: vec!["https://metron.cloud/issue/saga-1-2012".into()],
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("anything.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.issue_id, Some(10));
        assert_eq!(r.method, MatchMethod::ComicInfoWebMetron);
        assert_eq!(r.confidence, 1.0);
    }

    #[test]
    fn tier1_cv_wins_over_metron() {
        let series = vec![series(1, Some(42), "Saga", Some(2012))];
        let issues = vec![Issue {
            id: 10,
            series_id: 1,
            cv_id: Some(99),
            metron_id: Some("saga-1-2012".into()),
            number: IssueNumber::new("1"),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        }];
        let ci = ComicInfo {
            web: vec![
                "https://metron.cloud/issue/saga-1-2012".into(),
                "https://comicvine.gamespot.com/issue/4000-99/".into(),
            ],
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("x.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.method, MatchMethod::ComicInfoWebCv);
    }

    #[test]
    fn tier1_cv_url_present_but_no_matching_issue_falls_through() {
        // CV URL points to an issue we don't have in our DB. Should NOT return
        // Tier 1 match; should try Tier 2.
        let series = vec![series(1, Some(42), "Saga", Some(2012))];
        let issues = vec![issue(10, 1, Some(99), "1")];
        let ci = ComicInfo {
            series: Some("Saga".into()),
            number: Some("1".into()),
            web: vec!["https://comicvine.gamespot.com/issue/4000-77777/".into()],
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("x.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert_eq!(r.issue_id, Some(10));
    }

    // -------- Tier 2 --------

    #[test]
    fn tier2_exact_series_match_owned_confidence() {
        let series = vec![series(1, None, "The Walking Dead", Some(2003))];
        let issues = vec![issue(10, 1, None, "1")];
        let ci = ComicInfo {
            series: Some("The Walking Dead".into()),
            number: Some("1".into()),
            volume: Some(2003),
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("x.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert_eq!(r.issue_id, Some(10));
        assert!(r.confidence >= DEFAULT_MATCH_THRESHOLD);
    }

    #[test]
    fn tier2_typo_in_series_lands_in_needs_review_zone() {
        let series = vec![series(1, None, "The Walking Dead", Some(2003))];
        let issues = vec![issue(10, 1, None, "1")];
        let ci = ComicInfo {
            // Single-character typo. Levenshtein keeps score above 0.65 but
            // below 0.85.
            series: Some("Wlking Dead".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("x.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert_eq!(r.issue_id, Some(10));
        assert!(
            (NEEDS_REVIEW_FLOOR..1.0).contains(&r.confidence),
            "expected needs_review zone, got {}",
            r.confidence
        );
    }

    #[test]
    fn tier2_below_floor_falls_through_to_tier3() {
        // ComicInfo series is gibberish — Tier 2 won't get above 0.65.
        // Filename gives the right answer, so Tier 3 picks it up.
        let series = vec![series(1, None, "The Walking Dead", Some(2003))];
        let issues = vec![issue(10, 1, None, "1")];
        let ci = ComicInfo {
            series: Some("ZZZZZZZZZ".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(
            &ctx("The Walking Dead 1 (2003).cbz", Some(&ci), &ps),
            &series,
            &issues,
            DEFAULT_MATCH_THRESHOLD,
        );
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
    }

    #[test]
    fn tier2_falls_through_when_series_matches_but_issue_number_missing() {
        let series = vec![series(1, None, "Saga", Some(2012))];
        // We have issue #1 but ComicInfo says #99.
        let issues = vec![issue(10, 1, None, "1")];
        let ci = ComicInfo {
            series: Some("Saga".into()),
            number: Some("99".into()),
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("Saga 99.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        // No issue #99 anywhere → unmatched.
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert!(r.issue_id.is_none());
    }

    #[test]
    fn tier2_volume_tiebreaks_two_identical_series() {
        let series = vec![
            series(1, None, "Spider-Man", Some(1990)),
            series(2, None, "Spider-Man", Some(2014)),
        ];
        let issues = vec![
            issue(10, 1, None, "1"),
            issue(20, 2, None, "1"),
        ];
        let ci = ComicInfo {
            series: Some("Spider-Man".into()),
            number: Some("1".into()),
            volume: Some(2014),
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("x.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.issue_id, Some(20), "expected 2014 series to win on volume tie-break");
    }

    #[test]
    fn tier2_id_ascending_tiebreaks_when_no_volume() {
        let series = vec![
            series(1, None, "Spider-Man", Some(1990)),
            series(2, None, "Spider-Man", Some(2014)),
        ];
        let issues = vec![issue(10, 1, None, "1"), issue(20, 2, None, "1")];
        let ci = ComicInfo {
            series: Some("Spider-Man".into()),
            number: Some("1".into()),
            // No volume — no year tie-break information.
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("x.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.issue_id, Some(10), "expected lower-id series to win");
    }

    #[test]
    fn tier2_handles_leading_zero_issue_numbers() {
        let series = vec![series(1, None, "Saga", Some(2012))];
        let issues = vec![issue(10, 1, None, "1")];
        let ci = ComicInfo {
            series: Some("Saga".into()),
            number: Some("001".into()),
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(&ctx("x.cbz", Some(&ci), &ps), &series, &issues, DEFAULT_MATCH_THRESHOLD);
        assert_eq!(r.issue_id, Some(10));
    }

    // -------- Tier 3 --------

    #[test]
    fn tier3_clean_filename_caps_at_ceiling() {
        let series = vec![series(1, None, "Saga", Some(2012))];
        let issues = vec![issue(10, 1, None, "1")];
        let ps = default_patterns();
        let r = match_file(
            &ctx("Saga 1 (2012).cbz", None, &ps),
            &series,
            &issues,
            DEFAULT_MATCH_THRESHOLD,
        );
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
        assert!(
            (r.confidence - FILENAME_CONFIDENCE_CEILING).abs() < 1e-9,
            "expected ceiling 0.90, got {}",
            r.confidence
        );
    }

    #[test]
    fn tier3_unparseable_filename_returns_unmatched() {
        let series = vec![series(1, None, "Saga", Some(2012))];
        let issues = vec![issue(10, 1, None, "1")];
        let ps = default_patterns();
        let r = match_file(
            &ctx("random gibberish.txt", None, &ps),
            &series,
            &issues,
            DEFAULT_MATCH_THRESHOLD,
        );
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert!(r.issue_id.is_none());
    }

    #[test]
    fn tier3_parses_but_series_not_in_watchlist_returns_unmatched() {
        let series = vec![series(1, None, "Saga", Some(2012))];
        let issues = vec![issue(10, 1, None, "1")];
        let ps = default_patterns();
        let r = match_file(
            &ctx("Some Other Comic 1 (2012).cbz", None, &ps),
            &series,
            &issues,
            DEFAULT_MATCH_THRESHOLD,
        );
        assert_eq!(r.method, MatchMethod::Unmatched);
    }

    // -------- Cross-cutting --------

    #[test]
    fn no_match_with_empty_pools() {
        let ps = default_patterns();
        let r = match_file(
            &ctx("Saga 1 (2012).cbz", None, &ps),
            &[],
            &[],
            DEFAULT_MATCH_THRESHOLD,
        );
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert_eq!(r.confidence, 0.0);
        assert!(r.issue_id.is_none());
    }

    #[test]
    fn threshold_boundary_at_exact_value_classifies_as_owned() {
        // Verify the spec's `score >= threshold` rule via classify_status.
        use crate::file::classify_status;
        let r = MatchResult {
            issue_id: Some(7),
            method: MatchMethod::ComicInfoXml,
            confidence: 0.85,
        };
        let s = classify_status(r.issue_id, r.confidence, r.method, 0.85);
        assert_eq!(s, crate::FileStatus::Owned);
    }

    #[test]
    fn full_cascade_prefers_higher_tier_even_when_lower_tier_scores_higher() {
        // Tier 2 finds the series with a typo (score ~0.7 = needs_review zone).
        // Tier 3 would score 0.90 (clean filename → cap). Tier 2 wins because
        // it's a higher tier.
        let series = vec![series(1, None, "Saga", Some(2012))];
        let issues = vec![issue(10, 1, None, "1")];
        let ci = ComicInfo {
            // 4 chars, 1 substitution → Levenshtein 0.75, above 0.65 floor.
            series: Some("Saqa".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let ps = default_patterns();
        let r = match_file(
            &ctx("Saga 1 (2012).cbz", Some(&ci), &ps),
            &series,
            &issues,
            DEFAULT_MATCH_THRESHOLD,
        );
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert!(
            r.confidence < FILENAME_CONFIDENCE_CEILING,
            "expected Tier 2 confidence below filename ceiling, got {}",
            r.confidence
        );
    }
}
