//! Tier 2 + Tier 3 of the match cascade.
//!
//! Tier 1 (`<Web>` URL extraction) is the caller's responsibility — it needs
//! direct DB access to look up a CV / Metron issue ID, which `longbox-core`
//! cannot have. See [`crate::comicinfo::extract_cv_issue_id_from_url`] and
//! [`crate::comicinfo::extract_metron_issue_id_from_url`] for the per-URL
//! atoms the scanner uses to orchestrate Tier 1.
//!
//! This module handles:
//!
//! - **Tier 2** — ComicInfo `<Series>` + `<Number>`. Series text is compared
//!   against candidate `sort_title`s in normalized space via
//!   [`crate::similarity::similarity`]. Confidence = similarity score in
//!   `[0.65, 1.0)`. Below 0.65, fall through to Tier 3.
//! - **Tier 3** — filename-parse `series_title` + `number`. Same machinery,
//!   but confidence is capped at [`crate::FILENAME_CONFIDENCE_CEILING`]
//!   (0.90) so embedded metadata always wins a tie.
//!
//! No match in either tier → `MatchMethod::Unmatched`, confidence 0.0. The
//! matcher returns raw confidence only; classification (`owned` vs
//! `needs_review` vs `unmatched`) is the caller's responsibility via
//! [`crate::classify_status`].

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::comicinfo::ComicInfo;
use crate::file::MatchMethod;
use crate::filename::ParsedFilename;
use crate::issue::{Issue, IssueNumber};
use crate::normalize::normalize_title;
use crate::series::Series;
use crate::similarity::similarity;
use crate::{FILENAME_CONFIDENCE_CEILING, NEEDS_REVIEW_FLOOR};

/// A series and the issues belonging to it, fetched from the DB by the
/// scanner (or another caller) and handed to [`match_file`].
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub series: Series,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchResult {
    pub issue_id: Option<i64>,
    pub method: MatchMethod,
    pub confidence: f64,
}

/// Tier 2 → Tier 3 cascade against a pre-fetched candidate pool. Tier 1
/// (`<Web>` URL → issue lookup) is the caller's responsibility — call this
/// only after Tier 1 has failed to produce a match.
///
/// Returns the strongest non-failing result. `Unmatched` (confidence 0.0)
/// when neither tier produces a score ≥ [`NEEDS_REVIEW_FLOOR`].
pub fn match_file(
    comic_info: Option<&ComicInfo>,
    filename_parse: Option<&ParsedFilename>,
    candidates: &[Candidate],
) -> MatchResult {
    if let Some(result) = tier2_comicinfo(comic_info, candidates) {
        return result;
    }
    if let Some(result) = tier3_filename(filename_parse, candidates) {
        return result;
    }
    MatchResult {
        issue_id: None,
        method: MatchMethod::Unmatched,
        confidence: 0.0,
    }
}

fn tier2_comicinfo(
    comic_info: Option<&ComicInfo>,
    candidates: &[Candidate],
) -> Option<MatchResult> {
    let ci = comic_info?;
    let series_text = ci.series.as_deref()?;
    let number_text = ci.number.as_deref()?;
    match_in_candidates(
        series_text,
        number_text,
        ci.year,
        candidates,
        // No ceiling for ComicInfo matches — they go up to 1.0.
        f64::INFINITY,
        MatchMethod::ComicInfoXml,
    )
}

fn tier3_filename(
    filename_parse: Option<&ParsedFilename>,
    candidates: &[Candidate],
) -> Option<MatchResult> {
    let fp = filename_parse?;
    match_in_candidates(
        &fp.series_title,
        &fp.number,
        fp.year,
        candidates,
        FILENAME_CONFIDENCE_CEILING,
        MatchMethod::FilenameRegex,
    )
}

fn match_in_candidates(
    title_text: &str,
    number_text: &str,
    year_hint: Option<i32>,
    candidates: &[Candidate],
    confidence_ceiling: f64,
    method: MatchMethod,
) -> Option<MatchResult> {
    let (score, candidate) = best_candidate_match(title_text, year_hint, candidates)?;
    let confidence = score.min(confidence_ceiling);
    if confidence < NEEDS_REVIEW_FLOOR {
        return None;
    }
    let needle = IssueNumber::new(number_text);
    let issue = candidate
        .issues
        .iter()
        .find(|i| i.number.matches(&needle))?;
    Some(MatchResult {
        issue_id: Some(issue.id),
        method,
        confidence,
    })
}

/// Score every candidate against `title_text` (after normalization), return
/// the highest-scoring one. Tie-break: prefer candidates whose
/// `series.start_year` equals `year_hint` (only on bit-equal scores, per the
/// agreed rule). Final tie-break: lower `series.id` (first-added wins).
fn best_candidate_match<'a>(
    title_text: &str,
    year_hint: Option<i32>,
    candidates: &'a [Candidate],
) -> Option<(f64, &'a Candidate)> {
    if candidates.is_empty() {
        return None;
    }
    let normalized = normalize_title(title_text);

    // Apply `normalize_title` to BOTH sides at comparison time. The
    // stored `sort_title` is supposed to be normalized at insert time,
    // but real-world data shows ~50% of catalog rows have un-normalized
    // sort_titles (mixed case, etc.) from historical paths that
    // skipped normalization. Symmetric normalization here is durable
    // against that drift — without it `Y The Last Man` filenames score
    // ~0.71 against the stored `Y The Last Man` sort_title (case
    // mismatch destroys jaccard), falling below the 0.65 needs-review
    // floor and dead-ending as unmatched.
    let mut scored: Vec<(f64, &Candidate)> = candidates
        .iter()
        .map(|c| {
            (
                similarity(&normalized, &normalize_title(&c.series.sort_title)),
                c,
            )
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                let a_year = year_hint.is_some() && a.1.series.start_year == year_hint;
                let b_year = year_hint.is_some() && b.1.series.start_year == year_hint;
                b_year.cmp(&a_year)
            })
            // Prefer the candidate with more issues. When two series
            // share a title and neither matches the year hint (common:
            // NZB filenames carry the scan year, not the series
            // start_year), the larger catalog entry is almost always
            // the correct one — the smaller is typically a 1-issue
            // stub from an earlier import that the user hasn't pruned.
            // Live repro: "Sam and Twitch Case Files" had id=381 (2025,
            // 1 issue) vs id=918 (2024, 24 issues); files with a 2026
            // scan-year hint kept losing to the stub, sat in /watch/
            // indefinitely.
            .then_with(|| b.1.issues.len().cmp(&a.1.issues.len()))
            // Lower id as the final determinism guarantee — only
            // reached when title similarity, year-hint match, AND
            // issue count are all equal.
            .then_with(|| a.1.series.id.cmp(&b.1.series.id))
    });

    scored.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEFAULT_MATCH_THRESHOLD;

    fn series(id: i64, title: &str, year: Option<i32>) -> Series {
        Series::new(id, None, title, year)
    }

    fn issue(id: i64, series_id: i64, number: &str) -> Issue {
        Issue {
            id,
            series_id,
            cv_id: None,
            metron_id: None,
            number: IssueNumber::new(number),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        }
    }

    fn candidate(s: Series, issues: Vec<Issue>) -> Candidate {
        Candidate { series: s, issues }
    }

    // -------- Tier 2 (ComicInfo) --------

    #[test]
    fn tier2_exact_series_match_owned_confidence() {
        let s = series(1, "The Walking Dead", Some(2003));
        let issues = vec![issue(10, 1, "1")];
        let candidates = vec![candidate(s, issues)];
        let ci = ComicInfo {
            series: Some("The Walking Dead".into()),
            number: Some("1".into()),
            year: Some(2003),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert_eq!(r.issue_id, Some(10));
        assert!(r.confidence >= DEFAULT_MATCH_THRESHOLD);
    }

    #[test]
    fn tier2_typo_in_series_lands_in_needs_review_zone() {
        let s = series(1, "The Walking Dead", Some(2003));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Wlking Dead".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
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
        let s = series(1, "The Walking Dead", Some(2003));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("ZZZZZZZZZ".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let parsed = ParsedFilename {
            series_title: "The Walking Dead".into(),
            number: "1".into(),
            volume: None,
            year: Some(2003),
            title: None,
            pattern_id: 1,
        };
        let r = match_file(Some(&ci), Some(&parsed), &candidates);
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
    }

    #[test]
    fn tier2_falls_through_when_series_matches_but_number_missing() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Saga".into()),
            number: Some("99".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert!(r.issue_id.is_none());
    }

    #[test]
    fn tier2_year_tiebreaks_two_identical_series() {
        let candidates = vec![
            candidate(series(1, "Spider-Man", Some(1990)), vec![issue(10, 1, "1")]),
            candidate(series(2, "Spider-Man", Some(2014)), vec![issue(20, 2, "1")]),
        ];
        let ci = ComicInfo {
            series: Some("Spider-Man".into()),
            number: Some("1".into()),
            year: Some(2014),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
        assert_eq!(
            r.issue_id,
            Some(20),
            "2014 series should win on year tie-break"
        );
    }

    #[test]
    fn tier2_id_ascending_tiebreaks_when_no_year() {
        let candidates = vec![
            candidate(series(1, "Spider-Man", Some(1990)), vec![issue(10, 1, "1")]),
            candidate(series(2, "Spider-Man", Some(2014)), vec![issue(20, 2, "1")]),
        ];
        let ci = ComicInfo {
            series: Some("Spider-Man".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
        assert_eq!(r.issue_id, Some(10), "lower-id wins when no year hint");
    }

    #[test]
    fn tiebreak_prefers_more_issues_over_lower_id() {
        // Live repro: "Sam and Twitch Case Files" had id=381
        // (start_year=2025, 1 issue) and id=918 (start_year=2024, 24
        // issues). Files arrived with a 2026 scan-year hint so neither
        // matched on year. Pre-fix the lower id won and the
        // 1-issue stub didn't have the file's issue number — files
        // sat in /watch/ forever. Post-fix the larger catalog entry
        // wins.
        let small_id_few_issues = candidate(
            series(381, "Sam and Twitch Case Files", Some(2025)),
            vec![issue(10, 381, "1")],
        );
        let mut bigger_issues = Vec::new();
        for n in 1..=24 {
            bigger_issues.push(issue(100 + n, 918, &n.to_string()));
        }
        let large_id_many_issues =
            candidate(series(918, "Sam and Twitch Case Files", Some(2024)), bigger_issues);
        let candidates = vec![small_id_few_issues, large_id_many_issues];
        let ci = ComicInfo {
            series: Some("Sam and Twitch Case Files".into()),
            number: Some("23".into()),
            year: Some(2026), // matches neither start_year
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
        // The 24-issue series carries #23; the 1-issue stub does
        // not. Pre-fix this returned Unmatched (lower-id won,
        // didn't have #23); post-fix issue_id is the catalog #23.
        assert_eq!(r.issue_id, Some(100 + 23));
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
    }

    #[test]
    fn tiebreak_year_still_beats_issue_count() {
        // Year-hint match is a stronger signal than issue count — a
        // 24-issue catalog entry must NOT win over the 1-issue entry
        // when the year hint exactly matches the 1-issue entry's
        // start_year. Issue-count tiebreak only kicks in when year
        // hint doesn't discriminate.
        let small_with_year_match = candidate(
            series(381, "Sam and Twitch Case Files", Some(2025)),
            vec![issue(10, 381, "1")],
        );
        let mut bigger_issues = Vec::new();
        for n in 1..=24 {
            bigger_issues.push(issue(100 + n, 918, &n.to_string()));
        }
        let large_without_year_match = candidate(
            series(918, "Sam and Twitch Case Files", Some(2024)),
            bigger_issues,
        );
        let candidates = vec![small_with_year_match, large_without_year_match];
        let ci = ComicInfo {
            series: Some("Sam and Twitch Case Files".into()),
            number: Some("1".into()),
            year: Some(2025), // matches start_year=2025 only
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
        assert_eq!(
            r.issue_id,
            Some(10),
            "year-hint match must outrank issue count"
        );
    }

    #[test]
    fn tier2_handles_leading_zero_issue_numbers() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Saga".into()),
            number: Some("001".into()),
            ..Default::default()
        };
        let r = match_file(Some(&ci), None, &candidates);
        assert_eq!(r.issue_id, Some(10));
    }

    // -------- Tier 3 (filename) --------

    #[test]
    fn tier3_clean_filename_caps_at_ceiling() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Saga".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(None, Some(&parsed), &candidates);
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
        assert!(
            (r.confidence - FILENAME_CONFIDENCE_CEILING).abs() < 1e-9,
            "expected ceiling 0.90, got {}",
            r.confidence
        );
    }

    #[test]
    fn tier3_unmatched_when_no_candidates() {
        let parsed = ParsedFilename {
            series_title: "Saga".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(None, Some(&parsed), &[]);
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert!(r.issue_id.is_none());
    }

    #[test]
    fn tier3_unmatched_when_series_unrecognized() {
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Some Other Comic".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(None, Some(&parsed), &candidates);
        assert_eq!(r.method, MatchMethod::Unmatched);
    }

    // -------- Cross-cutting --------

    #[test]
    fn no_match_when_both_inputs_are_none() {
        let r = match_file(None, None, &[]);
        assert_eq!(r.method, MatchMethod::Unmatched);
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn full_cascade_prefers_higher_tier_even_when_lower_scores_higher() {
        // Tier 2: typo in ComicInfo → ~0.7 confidence (needs_review zone).
        // Tier 3: clean filename → would score ~1.0 but capped at 0.90.
        // Tier 2 still wins because it ran first.
        let s = series(1, "Saga", Some(2012));
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let ci = ComicInfo {
            series: Some("Saqa".into()),
            number: Some("1".into()),
            ..Default::default()
        };
        let parsed = ParsedFilename {
            series_title: "Saga".into(),
            number: "1".into(),
            volume: None,
            year: Some(2012),
            title: None,
            pattern_id: 2,
        };
        let r = match_file(Some(&ci), Some(&parsed), &candidates);
        assert_eq!(r.method, MatchMethod::ComicInfoXml);
        assert!(
            r.confidence < FILENAME_CONFIDENCE_CEILING,
            "expected Tier 2 confidence below filename ceiling, got {}",
            r.confidence
        );
    }

    #[test]
    fn symmetric_normalize_rescues_un_normalized_sort_title() {
        // Live data shows ~50% of catalog rows have un-normalized
        // sort_titles (mixed case, etc.) — `Series::new` builds them
        // via `normalize_title` for new rows, but historical paths
        // (older inserts, batch imports) stored them raw. Without
        // symmetric normalization at match time, a filename hint
        // `Y The Last Man` (normalized to `y the last man`) scored
        // ~0.71 against a stored `Y The Last Man` sort_title — below
        // the 0.85 owned threshold, sometimes below the 0.65
        // needs-review floor, dead-ending as unmatched. With the
        // symmetric normalize this scores 1.0.
        let s = Series {
            id: 1,
            cv_id: None,
            metron_id: None,
            title: "Y The Last Man".into(),
            sort_title: "Y The Last Man".into(), // un-normalized!
            start_year: Some(2002),
            publisher: None,
            description: None,
            cover_url: None,
        };
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Y The Last Man".into(),
            number: "1".into(),
            volume: None,
            year: Some(2002),
            title: None,
            pattern_id: 4,
        };
        let r = match_file(None, Some(&parsed), &candidates);
        assert_eq!(r.method, MatchMethod::FilenameRegex);
        assert_eq!(r.issue_id, Some(10));
        assert!(
            (r.confidence - FILENAME_CONFIDENCE_CEILING).abs() < 1e-9,
            "expected ceiling 0.90 from a 1.0 raw similarity, got {}",
            r.confidence
        );
    }

    #[test]
    fn nfc_normalization_makes_decomposed_unicode_match_precomposed() {
        // A filename pulled from macOS HFS+ stores `é` as decomposed
        // `e` + combining-acute. The catalog's `sort_title` (built from
        // CV's NFC-encoded data) carries the precomposed `é`. Before
        // NFC in `normalize_title` the two compared as different
        // strings; jaccard treated `pérez` (NFD) and `pérez` (NFC) as
        // disjoint tokens.
        let s = Series {
            id: 1,
            cv_id: None,
            metron_id: None,
            title: "Pérez".into(),         // precomposed
            sort_title: "pérez".into(),    // precomposed (NFC)
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        };
        let candidates = vec![candidate(s, vec![issue(10, 1, "1")])];
        let parsed = ParsedFilename {
            series_title: "Pe\u{0301}rez".into(), // decomposed (NFD)
            number: "1".into(),
            volume: None,
            year: None,
            title: None,
            pattern_id: 4,
        };
        let r = match_file(None, Some(&parsed), &candidates);
        assert_eq!(r.issue_id, Some(10));
    }

    #[test]
    fn by_author_attribution_strip_makes_verbose_filename_match_terse_catalog() {
        // Real case: ComicInfo / filename carries `Stillwater by
        // Zdarsky & Pérez` (Amazon-scraped verbose form) but the
        // catalog row is just `Stillwater`. Token-set jaccard would
        // score 1/4 = 0.25 without the strip. The `" by ..."` strip
        // in `normalize_title` collapses both sides to `stillwater`.
        let s = Series {
            id: 1,
            cv_id: None,
            metron_id: None,
            title: "Stillwater".into(),
            sort_title: "stillwater".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        };
        let candidates = vec![candidate(s, vec![issue(10, 1, "12")])];
        let parsed = ParsedFilename {
            series_title: "Stillwater by Zdarsky & Pérez".into(),
            number: "12".into(),
            volume: None,
            year: None,
            title: None,
            pattern_id: 4,
        };
        let r = match_file(None, Some(&parsed), &candidates);
        assert_eq!(r.issue_id, Some(10));
    }

    #[test]
    fn threshold_boundary_at_exact_value_classifies_as_owned() {
        // classify_status is the post-hoc step. Just verify it's reachable
        // from a matcher result.
        use crate::file::classify_status;
        let r = MatchResult {
            issue_id: Some(7),
            method: MatchMethod::ComicInfoXml,
            confidence: 0.85,
        };
        let s = classify_status(r.issue_id, r.confidence, r.method, 0.85);
        assert_eq!(s, crate::FileStatus::Owned);
    }
}
