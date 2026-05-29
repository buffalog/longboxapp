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
use std::sync::OnceLock;

use longbox_core::similarity::similarity;
use longbox_core::{normalize_title, parse_filename, ParsedFilename, ParsingPattern};
use regex::Regex;

use crate::types::{ArchiveFormat, Release};

/// Bounded plausible-publishing-year window for the Scene normalizer's
/// year-wrap step. 1950 floor doubles as an issue-number guard — no real
/// series has reached issue #1950, so an issue number that happens to be
/// a 4-digit value (e.g. Action Comics #1000) won't get false-wrapped as
/// a year. 2039 ceiling covers near-future solicitations. Widening only
/// warranted if Golden-Age original-year tags surface as real misses.
static SCENE_YEAR_RE: OnceLock<Regex> = OnceLock::new();
fn scene_year_re() -> &'static Regex {
    SCENE_YEAR_RE.get_or_init(|| Regex::new(r"\b(19[5-9]\d|20[0-3]\d)\b").unwrap())
}

/// Scene-format → canonical-filename shape adapter (Bug 3a). Scene
/// release titles are dot-separated, year-token-bare, extension-less:
/// `Beware.the.Eye.of.Odin.001.2022.Digital.Mephisto-Empire`. The
/// canonical parser cascade expects space-separated, parenthesized-year,
/// extension-bearing inputs: `Beware the Eye of Odin 001 (2022).cbz`.
/// Three transforms close the gap:
///
/// 1. **Dots → spaces, blanket.** Title-internal dots
///    (`S.W.O.R.D.`, `G.I.Joe`) lose their structure but the matcher's
///    `normalize_title` reduces both sides to the same punctuation-free
///    form, so similarity scoring isn't affected.
///
/// 2. **Wrap the rightmost in-range bare year as `(YYYY)`.** Scene
///    format places the release year last, after the issue number, so
///    rightmost = release year. Wrapping only the rightmost match
///    preserves title-internal years (`2000 AD` is a real series whose
///    name contains a year-range number — wrapping it as the year would
///    break the parse).
///
/// 3. **Append `.cbz`** so the trailing `\.(?i:cbz|cbr|cb7)$` anchor in
///    every pattern matches. The fact that Scene NZBs aren't actually
///    CBZs is irrelevant — we're feeding the *parser*, not the
///    downloader.
///
/// Called as a fall-back from [`filter_by_series_title`] only after
/// `parse_filename` returns `None` on the raw title — canonical inputs
/// short-circuit before reaching here, avoiding any double-wrap of
/// already-parenthesized years.
pub fn normalize_scene_title(input: &str) -> String {
    // 1. dots → spaces
    let with_spaces = input.replace('.', " ");
    // 2. rightmost-only year wrap
    let re = scene_year_re();
    let wrapped = if let Some(last) = re.find_iter(&with_spaces).last() {
        let (start, end) = (last.start(), last.end());
        let mut out = String::with_capacity(with_spaces.len() + 2);
        out.push_str(&with_spaces[..start]);
        out.push('(');
        out.push_str(&with_spaces[start..end]);
        out.push(')');
        out.push_str(&with_spaces[end..]);
        out
    } else {
        with_spaces
    };
    // 3. append .cbz
    format!("{wrapped}.cbz")
}

/// Run the parser on the raw title; fall back to the Scene normalizer
/// if the raw parse fails. The canonical case (raw title parses) never
/// reaches the normalizer, so already-parenthesized years can't get
/// double-wrapped.
fn parse_release_title(title: &str, patterns: &[ParsingPattern]) -> Option<ParsedFilename> {
    if let Some(parsed) = parse_filename(title, patterns) {
        return Some(parsed);
    }
    parse_filename(&normalize_scene_title(title), patterns)
}

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
        let Some(parsed) = parse_release_title(&release.title, patterns) else {
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

    /// Score-first ordering guard. A pool that mixes (a) a release
    /// scoring above threshold but failing the year gate with (b) a
    /// release passing the year gate but scoring below threshold should
    /// land as silent no-match — the year-mismatched release proves the
    /// indexer has the right series, just wrong volume. A naive
    /// year-first refactor would mark this as a series-mismatch because
    /// best_similarity would only see (b)'s low score. This locks the
    /// corrected mid-implementation behavior in place.
    #[test]
    fn filter_year_mismatch_silent_holds_even_with_below_threshold_companion() {
        let patterns = default_patterns();
        let pool = vec![
            // (a) Right series, wrong year — year filter rejects silently.
            release("Wolverine 5 (2024).cbz", Some(50)),
            // (b) Wrong series, year matches — similarity rejects.
            release("Wolverine MAX 5 (1982).cbz", Some(20)),
        ];
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
            "year-rejected (a) proves indexer has right series — must stay silent. got {:?}",
            outcome.mismatch
        );
    }

    // -------- normalize_scene_title (Bug 3a) --------
    //
    // Baseline: the 11 archaeology cases that parsed cleanly under the
    // normalizer. These lock the contract between normalize_scene_title
    // and parse_filename — a future change to either side that breaks
    // these will fail loudly. The 2 graceful-failure cases lock the
    // unparseable-fall-through behavior.

    fn check_normalized_parses_to(
        raw: &str,
        expected_series: &str,
        expected_number: &str,
        expected_year: Option<i32>,
    ) {
        let patterns = default_patterns();
        let parsed = parse_release_title(raw, &patterns)
            .unwrap_or_else(|| panic!("Bug 3a: expected to parse {raw:?}"));
        assert_eq!(
            parsed.series_title.to_lowercase(),
            expected_series.to_lowercase(),
            "series for {raw:?}"
        );
        assert_eq!(parsed.number, expected_number, "number for {raw:?}");
        assert_eq!(parsed.year, expected_year, "year for {raw:?}");
    }

    #[test]
    fn normalize_real_wolverine_patch() {
        check_normalized_parses_to(
            "Wolverine.-.Patch.005.2022.Digital.Zone-Empire",
            "Wolverine - Patch",
            "005",
            Some(2022),
        );
    }

    #[test]
    fn normalize_real_life_of_wolverine_infinity_comic() {
        check_normalized_parses_to(
            "Life.of.Wolverine.-.Infinity.Comic.005.2022.digital-mobile.Empire",
            "Life of Wolverine - Infinity Comic",
            "005",
            Some(2022),
        );
    }

    #[test]
    fn normalize_real_hello_darkness() {
        check_normalized_parses_to(
            "Hello.Darkness.001.2024.digital.Son.of.Ultron-Empire",
            "Hello Darkness",
            "001",
            Some(2024),
        );
    }

    #[test]
    fn normalize_real_beware_eye_of_odin_5_26_false_positive() {
        // The exact 5-26 wrong-grab. Under Bug 3a this NOW parses
        // (correctly extracting "Beware the Eye of Odin"), and the
        // similarity filter rejects it against requested "Odin".
        check_normalized_parses_to(
            "Beware.the.Eye.of.Odin.001.2022.Digital.Mephisto-Empire",
            "Beware the Eye of Odin",
            "001",
            Some(2022),
        );
    }

    #[test]
    fn normalize_real_thanos_death_notes_cross_result() {
        check_normalized_parses_to(
            "Thanos.-.Death.Notes.001.2023.Digital.Zone-Empire",
            "Thanos - Death Notes",
            "001",
            Some(2023),
        );
    }

    #[test]
    fn normalize_synth_number_leading_title() {
        check_normalized_parses_to(
            "20th.Century.Men.001.2022.Digital.Mephisto-Empire",
            "20th Century Men",
            "001",
            Some(2022),
        );
    }

    #[test]
    fn normalize_synth_article_multi_word() {
        check_normalized_parses_to(
            "The.Walking.Dead.100.2012.Digital.Zone-Empire",
            "The Walking Dead",
            "100",
            Some(2012),
        );
    }

    #[test]
    fn normalize_synth_vintage_number_leading() {
        check_normalized_parses_to(
            "100.Bullets.050.2002.Digital.Zone-Empire",
            "100 Bullets",
            "050",
            Some(2002),
        );
    }

    #[test]
    fn normalize_synth_title_internal_dots() {
        // S.W.O.R.D. — the title-internal-dots worst case. Dots → spaces
        // loses the structure but the matcher's normalize_title reduces
        // both "S.W.O.R.D." and "S W O R D" to identical normalized form,
        // so similarity scoring is preserved.
        check_normalized_parses_to(
            "S.W.O.R.D..001.2010.Digital.Zone-Empire",
            "S W O R D",
            "001",
            Some(2010),
        );
    }

    #[test]
    fn normalize_synth_short_word() {
        check_normalized_parses_to(
            "Sex.001.2013.Digital.Image-Empire",
            "Sex",
            "001",
            Some(2013),
        );
    }

    #[test]
    fn normalize_synth_sibling_under_scene_format() {
        // Confirms the Wolverine-MAX boundary case (sibling-series
        // collision) survives under Scene format — series extraction
        // yields "Wolverine MAX", which similarity vs "Wolverine"
        // scores below 0.75 the same way the canonical-format case did.
        check_normalized_parses_to(
            "Wolverine.MAX.001.2024.Digital.Zone-Empire",
            "Wolverine MAX",
            "001",
            Some(2024),
        );
    }

    /// **The 2000 AD fixture** — title containing a year-range number.
    /// Rightmost-year-only wrapping must wrap the release year (2023)
    /// and leave the title-internal 2000 alone. Global wrap would
    /// produce "(2000) AD" and break the parse.
    #[test]
    fn normalize_2000_ad_rightmost_year_only() {
        check_normalized_parses_to(
            "2000.AD.2350.2023.Digital.Zone-Empire",
            "2000 AD",
            "2350",
            Some(2023),
        );
    }

    /// **Graceful failure 1**: editorial annotation between number and
    /// year (the real G.I. Joe `Larry Hama Cut` case). Falls through to
    /// the unparseable→mismatch path. Deferred for a future normalizer-
    /// local strip-known-annotation-tokens step; NOT a parser-cascade
    /// change.
    #[test]
    fn normalize_gi_joe_edition_annotation_still_no_parse() {
        let patterns = default_patterns();
        let parsed = parse_release_title(
            "G.I.Joe.A.Real.American.Hero.001.Larry.Hama.Cut.2023.digital.Knight.Ripper-Empire",
            &patterns,
        );
        assert!(
            parsed.is_none(),
            "Edition-annotation-between-number-and-year: graceful unparseable expected. got {parsed:?}"
        );
    }

    /// **Graceful failure 2**: no year in title. Falls through to
    /// unparseable→mismatch — under-determined input, no year to anchor
    /// on.
    #[test]
    fn normalize_no_year_still_no_parse() {
        let patterns = default_patterns();
        let parsed =
            parse_release_title("Wolverine.005.Digital.Mephisto-Empire", &patterns);
        assert!(
            parsed.is_none(),
            "No-year Scene title: graceful unparseable expected. got {parsed:?}"
        );
    }

    /// Canonical filename input MUST NOT reach the normalizer — that'd
    /// double-wrap the year. Raw parse short-circuits the fall-back.
    #[test]
    fn normalize_canonical_input_uses_raw_parse_path() {
        check_normalized_parses_to(
            "Wolverine 005 (2024) (digital).cbz",
            "Wolverine",
            "005",
            Some(2024),
        );
    }

    // -------- End-to-end filter (Scene-format) --------

    /// Bug 3 + Bug 3a composed: the exact 5-26 wrong-grab now parses
    /// (via the normalizer), but the similarity filter rejects it on
    /// title-mismatch — diagnostic shape changes from "none parseable"
    /// (Bug 3) to "below threshold" (Bug 3a).
    #[test]
    fn filter_rejects_odin_false_positive_via_similarity_path_under_scene_format() {
        let patterns = default_patterns();
        let pool = vec![release(
            "Beware.the.Eye.of.Odin.001.2022.Digital.Mephisto-Empire",
            Some(24),
        )];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Odin",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert!(outcome.kept.is_empty(), "must reject the wrong-series grab");
        let diag = outcome.mismatch.expect("must produce a mismatch row");
        assert_eq!(diag.parseable_count, 1, "Scene title is now parseable via normalizer");
        let score = diag.best_similarity.expect("similarity scored");
        assert!(score < 0.5, "Odin vs Beware-the-Eye-of-Odin should score low, got {score}");
        let msg = diag.into_error_message("Odin", 0.75);
        assert!(
            msg.contains("below threshold"),
            "expected similarity-path wording, got {msg}"
        );
    }

    /// Bug 3a positive control: clean Scene-format Hello Darkness 001
    /// parses + accepts.
    #[test]
    fn filter_accepts_clean_scene_format_hello_darkness() {
        let patterns = default_patterns();
        let pool = vec![release(
            "Hello.Darkness.001.2024.digital.Son.of.Ultron-Empire",
            Some(50),
        )];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Hello Darkness",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        );
        assert_eq!(outcome.kept.len(), 1);
        assert!(outcome.mismatch.is_none());
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
