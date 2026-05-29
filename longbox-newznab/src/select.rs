//! Result selection — pick the single best release for an issue.
//!
//! Two stages:
//! - [`filter_by_series_title`] — Bug 3 (2026-05-28) pre-grab correctness
//!   filter. Re-parses each release's filename-shaped title via
//!   `longbox-core::parse_filename` (no parallel parser — reuses the
//!   patterns the catalog scanner uses), drops releases whose year
//!   disagrees with the requested year (silent — wrong volume), drops
//!   releases whose series segment fails `similarity(requested, parsed)`
//!   below the configured threshold (counted as a mismatch — wrong
//!   series), and reports a structured outcome the caller turns into
//!   either a pull-attempt mismatch row or a fall-through.
//! - [`select_best`] — sort the surviving pool by format (cbz>cbr) +
//!   grabs + recency, return the winner. Unchanged ranking contract.
//!
//! Both functions are pure — they take inputs by value/ref and own no I/O.

use std::cmp::Ordering;

use longbox_core::similarity::similarity;
use longbox_core::{normalize_title, parse_filename, ParsingPattern};

use crate::types::{ArchiveFormat, Release};

/// Outcome of [`filter_by_series_title`] — the kept pool plus, when the
/// kept pool is empty, an optional diagnostic for the caller's error
/// surfacing. `Some(diagnostic)` means series-mismatch (surface as a
/// `pull_attempts.status='mismatched'` row); `None` means the pool was
/// already empty or year-filter alone emptied it (silent fall-through to
/// the next indexer / next sweep, no row).
#[derive(Debug, Clone, PartialEq)]
pub struct FilterOutcome {
    /// Releases that passed both gates, ranked-eligible.
    pub kept: Vec<Release>,
    /// Populated when `kept` is empty AND at least one release was
    /// rejected for series-mismatch reasons (unparseable or below
    /// threshold). Year-only rejections leave this `None` so the engine
    /// can distinguish "wrong volume" (silent) from "wrong series"
    /// (surface).
    pub mismatch: Option<MismatchDiagnostic>,
}

/// Why an indexer's pool became empty post-filter, used to populate the
/// `pull_attempts.error_message` field. Field semantics mirror the four
/// rows of the Bug 3 kickoff table: `total_results` is what the indexer
/// returned; `parseable_count` is the subset `parse_filename` recognized
/// as comic releases; `best_similarity` is the highest score *among
/// parseable* releases, or `None` when nothing parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct MismatchDiagnostic {
    pub total_results: usize,
    pub parseable_count: usize,
    pub best_similarity: Option<f64>,
    pub best_series_segment: Option<String>,
}

impl MismatchDiagnostic {
    /// Render a human-readable error_message matching the four-row table
    /// in the Bug 3 kickoff. Caller stores this on the `pull_attempts`
    /// row.
    pub fn into_error_message(&self, requested_series_title: &str, threshold: f64) -> String {
        match (self.parseable_count, self.best_similarity) {
            (0, _) => format!(
                "indexer returned {} results, none parseable as a comic release",
                self.total_results,
            ),
            (parseable, Some(score)) => format!(
                "indexer returned {} results, {} parseable, best similarity {:.2} vs requested {:?} below threshold {:.2}",
                self.total_results, parseable, score, requested_series_title, threshold,
            ),
            // Parseable > 0 but best_similarity = None can't happen
            // (every parseable release scores), but the type allows it;
            // degrade to the all-unparseable shape rather than panic.
            (_, None) => format!(
                "indexer returned {} results, none parseable as a comic release",
                self.total_results,
            ),
        }
    }
}

/// Pre-grab correctness filter. Drops releases whose year disagrees with
/// the requested year (silent — different volume, retry next sweep), then
/// drops releases whose series segment scores below `threshold` against
/// the requested title in normalized space (mismatch — wrong series,
/// surface to the user).
///
/// `requested_year = None` skips the year filter entirely (subscription
/// has no year, e.g. a shallow series). `parsed.year = None` always
/// passes the year filter — newznab titles sometimes lack year stamps
/// and over-filtering wrong on `None` would over-reject. See the Bug 3
/// deferred derivative for the eventual single-candidate fallback
/// tightening.
pub fn filter_by_series_title(
    releases: Vec<Release>,
    patterns: &[ParsingPattern],
    requested_series_title: &str,
    requested_year: Option<i32>,
    threshold: f64,
) -> FilterOutcome {
    let total_results = releases.len();
    let requested_normalized = normalize_title(requested_series_title);

    let mut kept: Vec<Release> = Vec::new();
    let mut parseable_count = 0usize;
    let mut best_similarity: Option<f64> = None;
    let mut best_series_segment: Option<String> = None;

    for release in releases {
        let Some(parsed) = parse_filename(&release.title, patterns) else {
            continue; // unparseable — counted as wrong-series implicitly
        };
        parseable_count += 1;

        // Score similarity FIRST, before any short-circuit. The mismatch
        // decision below uses best_similarity to detect the "year-only
        // rejection" case (best ≥ threshold but kept is empty → silent);
        // skipping the score for year-rejected releases would lose that
        // signal and wrongly mark a year-mismatch as a series-mismatch.
        let parsed_normalized = normalize_title(&parsed.series_title);
        let score = similarity(&requested_normalized, &parsed_normalized);

        if best_similarity.map_or(true, |b| score > b) {
            best_similarity = Some(score);
            best_series_segment = Some(parsed.series_title.clone());
        }

        if score < threshold {
            continue; // series-mismatch — diagnostic-worthy
        }

        // Year gate — silent reject on disagreement, pass on absence.
        if let (Some(req_year), Some(rel_year)) = (requested_year, parsed.year) {
            if req_year != rel_year {
                continue;
            }
        }

        kept.push(release);
    }

    let mismatch = if kept.is_empty() && total_results > 0 {
        // Year-only rejection: at least one release scored above the
        // similarity threshold but every above-threshold release failed
        // the year filter. Silent fall-through, no diagnostic — the
        // user's subscription year disagrees with what the indexer
        // carries, not a series-naming problem.
        if best_similarity.is_some_and(|b| b >= threshold) {
            None
        } else {
            Some(MismatchDiagnostic {
                total_results,
                parseable_count,
                best_similarity,
                best_series_segment,
            })
        }
    } else {
        None
    };

    FilterOutcome { kept, mismatch }
}

/// Infer archive format from a release title. Newznab has no
/// structured format field; sniff the `.cbz` / `.cbr` substring.
pub fn archive_format(title: &str) -> ArchiveFormat {
    let lower = title.to_lowercase();
    if lower.contains(".cbz") {
        ArchiveFormat::Cbz
    } else if lower.contains(".cbr") {
        ArchiveFormat::Cbr
    } else {
        ArchiveFormat::Unknown
    }
}

fn format_rank(title: &str) -> u8 {
    match archive_format(title) {
        ArchiveFormat::Cbz => 0,
        ArchiveFormat::Cbr => 1,
        ArchiveFormat::Unknown => 2,
    }
}

/// Pick the best release from a non-prioritized pool, or `None` when
/// the pool is empty.
pub fn select_best(mut releases: Vec<Release>) -> Option<Release> {
    releases.sort_by(cmp_releases);
    releases.into_iter().next()
}

/// Ordering where "less" = "better" (sorts the winner to the front).
fn cmp_releases(a: &Release, b: &Release) -> Ordering {
    format_rank(&a.title)
        .cmp(&format_rank(&b.title))
        // higher grabs first
        .then_with(|| b.grabs.unwrap_or(0).cmp(&a.grabs.unwrap_or(0)))
        // more recent first — None (unknown date) sorts last
        .then_with(|| b.published.cmp(&a.published))
}

#[cfg(test)]
mod tests {
    use super::*;
    use longbox_core::filename::default_patterns;
    use time::macros::datetime;

    fn release(title: &str, grabs: Option<i64>) -> Release {
        Release {
            title: title.into(),
            nzb_url: format!("https://x/{title}"),
            guid: title.into(),
            published: None,
            size_bytes: None,
            grabs,
            category: None,
        }
    }

    // -------- archive_format --------

    #[test]
    fn detects_archive_format_from_title() {
        assert_eq!(archive_format("Wolverine 005.cbz"), ArchiveFormat::Cbz);
        assert_eq!(archive_format("Wolverine 005.CBR"), ArchiveFormat::Cbr);
        assert_eq!(archive_format("Wolverine 005"), ArchiveFormat::Unknown);
    }

    // -------- select_best (legacy ranking, unchanged) --------

    #[test]
    fn prefers_cbz_over_cbr_even_with_fewer_grabs() {
        let pool = vec![
            release("Wolverine 005.cbr", Some(100)),
            release("Wolverine 005.cbz", Some(5)),
        ];
        assert_eq!(select_best(pool).unwrap().title, "Wolverine 005.cbz");
    }

    #[test]
    fn within_same_format_prefers_higher_grabs() {
        let pool = vec![
            release("a.cbz", Some(5)),
            release("b.cbz", Some(80)),
            release("c.cbz", Some(40)),
        ];
        assert_eq!(select_best(pool).unwrap().title, "b.cbz");
    }

    #[test]
    fn recency_breaks_grab_ties() {
        let mut older = release("old.cbz", Some(10));
        older.published = Some(datetime!(2024-01-01 0:00 UTC));
        let mut newer = release("new.cbz", Some(10));
        newer.published = Some(datetime!(2025-01-01 0:00 UTC));
        let pool = vec![older, newer];
        assert_eq!(select_best(pool).unwrap().title, "new.cbz");
    }

    #[test]
    fn cbr_still_selectable_when_no_cbz() {
        let pool = vec![release("only.cbr", Some(1))];
        assert_eq!(select_best(pool).unwrap().title, "only.cbr");
    }

    #[test]
    fn empty_pool_is_none() {
        assert!(select_best(vec![]).is_none());
    }

    // -------- filter_by_series_title (Bug 3) --------

    /// The two false-positive grabs that surfaced in A.8 Scenario 1
    /// smoke (2026-05-26). These are the load-bearing regression cases.

    #[test]
    fn filter_rejects_odin_false_positive() {
        // Subscribed to "Odin", indexer returned "Beware the Eye of Odin".
        // Pre-Bug-3, select_best would have grabbed this and Phase B
        // would have either misfiled it or quarantined it to _unsorted/.
        let patterns = default_patterns();
        let pool = vec![release(
            "Beware the Eye of Odin 001 (2022) (digital).cbr",
            Some(50),
        )];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Odin",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert!(outcome.kept.is_empty(), "kept should be empty");
        let diag = outcome.mismatch.expect("must produce a mismatch row");
        assert_eq!(diag.total_results, 1);
        assert_eq!(diag.parseable_count, 1);
        let score = diag.best_similarity.unwrap();
        assert!(score < 0.5, "Odin vs Beware-Eye-of-Odin should score low, got {score}");
    }

    #[test]
    fn filter_rejects_darkness_false_positive() {
        // Subscribed to "The Darkness", indexer returned "Justice League".
        let patterns = default_patterns();
        let pool = vec![release(
            "Justice League - Road To Dark Crisis 001 (2022).cbz",
            Some(30),
        )];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "The Darkness",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert!(outcome.kept.is_empty());
        let diag = outcome.mismatch.expect("must produce a mismatch row");
        assert!(
            diag.best_similarity.unwrap() < 0.2,
            "darkness vs justice-league should be near zero, got {:?}",
            diag.best_similarity
        );
    }

    #[test]
    fn filter_accepts_clean_wolverine_match() {
        let patterns = default_patterns();
        let pool = vec![release("Wolverine 005 (2024) (digital).cbz", Some(100))];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Wolverine",
            Some(2024),
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert_eq!(outcome.kept.len(), 1);
        assert!(outcome.mismatch.is_none());
    }

    /// The sibling-series boundary case. At threshold 0.65 this passes
    /// (Wolverine MAX wrongly accepted); at 0.75 it correctly fails.
    /// Locks the 0.75 calibration in place.
    #[test]
    fn filter_rejects_sibling_series_at_default_threshold() {
        let patterns = default_patterns();
        let pool = vec![release("Wolverine MAX 1 (2024).cbz", Some(20))];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Wolverine",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD, // 0.75
        );
        assert!(outcome.kept.is_empty(), "Wolverine MAX must not pass at 0.75");
        let diag = outcome.mismatch.unwrap();
        let score = diag.best_similarity.unwrap();
        assert!(
            (0.60..0.75).contains(&score),
            "expected sibling boundary score 0.60-0.75, got {score}"
        );
    }

    #[test]
    fn filter_accepts_subtitle_with_hyphen_variant() {
        // "Wolverine: Origin" (colon dropped to space by normalize_title)
        // vs release "Wolverine - Origin 1 (2024).cbz" (hyphen preserved).
        // Should pass — same series, punctuation variant.
        let patterns = default_patterns();
        let pool = vec![release("Wolverine - Origin 1 (2024).cbz", Some(15))];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Wolverine: Origin",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert_eq!(outcome.kept.len(), 1, "subtitle variant should pass");
    }

    #[test]
    fn filter_drops_year_mismatch_silently() {
        // Subscribed to 1982 Wolverine, indexer returned 2024 Wolverine.
        // Both gates evaluate: series similarity = 1.0 (accept), but year
        // disagrees → silent drop, NO mismatch diagnostic.
        let patterns = default_patterns();
        let pool = vec![release("Wolverine 5 (2024).cbz", Some(10))];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Wolverine",
            Some(1982),
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert!(outcome.kept.is_empty());
        assert!(
            outcome.mismatch.is_none(),
            "year-only rejection must not surface a mismatch row — got {:?}",
            outcome.mismatch
        );
    }

    #[test]
    fn filter_year_none_in_release_passes_year_gate() {
        // Release without a year stamp — current design defers to similarity.
        // Subscribed 1982 Wolverine, release "Wolverine 5.cbz" — no year
        // captured by parser, so year gate passes. Series similarity = 1.0.
        // This is the "no-year wrong-volume residual" deferred item: the
        // release MIGHT be the wrong volume but we have no signal to reject.
        let patterns = default_patterns();
        let pool = vec![release("Wolverine 5.cbz", Some(10))];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Wolverine",
            Some(1982),
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert_eq!(outcome.kept.len(), 1);
    }

    #[test]
    fn filter_unparseable_releases_become_mismatch() {
        // A pool whose every release fails parse_filename surfaces as a
        // mismatch ("indexer returning junk for this query").
        let patterns = default_patterns();
        let pool = vec![
            release("Some random Linux ISO blob no comic structure", Some(1)),
            release("Another (totally) malformed nonsense", Some(1)),
        ];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Wolverine",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert!(outcome.kept.is_empty());
        let diag = outcome.mismatch.expect("all-unparseable → mismatch");
        assert_eq!(diag.total_results, 2);
        assert_eq!(diag.parseable_count, 0);
        assert!(diag.best_similarity.is_none());
        let msg = diag.into_error_message("Wolverine", 0.75);
        assert!(
            msg.contains("none parseable"),
            "expected unparseable wording, got {msg}"
        );
    }

    #[test]
    fn filter_mixed_unparseable_and_below_threshold_surfaces_score() {
        let patterns = default_patterns();
        let pool = vec![
            release("Pure junk no structure", Some(1)),
            release("Beware the Eye of Odin 001 (2022).cbr", Some(50)),
        ];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Odin",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        let diag = outcome.mismatch.expect("mismatch expected");
        assert_eq!(diag.total_results, 2);
        assert_eq!(diag.parseable_count, 1);
        assert!(diag.best_similarity.is_some());
        let msg = diag.into_error_message("Odin", 0.75);
        assert!(msg.contains("2 results"));
        assert!(msg.contains("1 parseable"));
        assert!(msg.contains("below threshold"));
    }

    #[test]
    fn filter_empty_pool_is_no_match_not_mismatch() {
        // An indexer that returned zero results: empty pool in, empty
        // outcome out, NO mismatch row (covered by the upstream
        // zero-results path in find_release_excluding).
        let patterns = default_patterns();
        let outcome = filter_by_series_title(
            Vec::new(),
            &patterns,
            "Wolverine",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert!(outcome.kept.is_empty());
        assert!(outcome.mismatch.is_none());
    }
}
