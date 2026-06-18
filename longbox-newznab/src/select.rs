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
/// Bracketed-tag NZB title → canonical-filename shape adapter.
/// NZBGeek and Prowlarr-via-newznab ship titles like:
///
/// ```text
/// Absolute Green Lantern 007 [2025] [digital-mobile] [Son of Ultron-Empire]
/// Absolute.Green.Lantern.008 [2026] [Webrip] [The Last Kryptonian-DCP]
/// ```
///
/// Square brackets for year + tags + group, dots-as-spaces in the
/// series prefix (sometimes), no extension. Neither the raw cascade
/// nor [`normalize_scene_title`] handles these — the scene
/// normalizer's rightmost-year-wrap would land the year token INSIDE
/// the brackets, producing `[(2025)]` which the filename parser
/// still rejects.
///
/// Approach (matches the user-stated spec from the Tier 4 bug):
///
/// 1. **Capture year** from the first `[YYYY]` bracket. Regex
///    requires the whole bracket to be exactly four digits — a
///    `[2025-Backup]` tag won't poison the year, only a bare
///    `[YYYY]` is recognized.
/// 2. **Strip all bracketed segments** wholesale.
/// 3. **Dots → spaces** so `Absolute.Green.Lantern.008` becomes the
///    space-separated form the filename parser expects.
/// 4. **Collapse whitespace** — bracket-stripping leaves runs of
///    spaces where the brackets used to sit; squash them.
/// 5. **Re-emit canonical** — `{cleaned} ({year}).cbz` when year was
///    captured, year-less `{cleaned}.cbz` otherwise. The yearless
///    form falls through to the file parser's catch-all / `(of N)
///    yearless` patterns; `(YYYY)` keeps the strict-year patterns
///    in play.
///
/// Called from [`parse_release_title`] between the `{title}.cbz`
/// pass and [`normalize_scene_title`] so bracketed titles get
/// claimed BEFORE the scene normalizer mangles them.
pub fn normalize_nzb_title(input: &str) -> String {
    let year_re = nzb_year_re();
    let year = year_re
        .captures(input)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_owned()));
    let bracket_re = nzb_bracket_re();
    let without_brackets = bracket_re.replace_all(input, " ");
    let dotted = without_brackets.replace('.', " ");
    let cleaned: String = dotted.split_whitespace().collect::<Vec<_>>().join(" ");
    match year {
        Some(y) => format!("{cleaned} ({y}).cbz"),
        None => format!("{cleaned}.cbz"),
    }
}

static NZB_YEAR_RE: OnceLock<Regex> = OnceLock::new();
fn nzb_year_re() -> &'static Regex {
    NZB_YEAR_RE.get_or_init(|| Regex::new(r"\[(\d{4})\]").unwrap())
}

static NZB_BRACKET_RE: OnceLock<Regex> = OnceLock::new();
fn nzb_bracket_re() -> &'static Regex {
    NZB_BRACKET_RE.get_or_init(|| Regex::new(r"\[[^\]]*\]").unwrap())
}

/// Scene-format → canonical-filename shape adapter, extended to also
/// handle fully dot-separated scene names with mini-series notation
/// (`Arcana.Royale.01.of.04.2025.digital.Son.of.Ultron-Empire`). The
/// load-bearing addition over the original four-step transform is
/// step 3: wrapping the `\d+ of \d+` substring as `\d+ (of \d+)` so
/// the parser's `\(of\s+\d+\)` matchers (default patterns id=11, 12,
/// 13) can claim the mini-series shape. Without that wrap, the bare
/// `01 of 04 (2025)` mis-captures as series=`... 01 of`, number=`04`
/// via pattern id=10's permissive tail.
///
/// Transforms in order:
///
///  1. **Strip trailing `-Group` suffix.** The last hyphenated single-
///     word token (e.g. `-Empire`, `-DCP`, `-LeDuch`) at the END of
///     the string gets removed. The regex is anchored so internal
///     hyphenated tokens (`digital-mobile`, `Spider-Man`) survive.
///
///  2. **Dots → spaces, blanket.** Title-internal dots
///     (`S.W.O.R.D.`, `G.I.Joe`) lose their structure but the
///     matcher's `normalize_title` reduces both sides to the same
///     punctuation-free form, so similarity scoring isn't affected.
///
///  3. **Wrap `\d+ of \d+` as `\d+ (of \d+)`.** Mini-series notation
///     in scene format is dot-separated (`01.of.04`); the parser
///     patterns expect parenthesized form (`01 (of 04)`). Whole-word
///     digit-of-digit match, so `Life of Wolverine` and similar
///     letter-flanked `of` tokens are untouched.
///
///  4. **Strip standalone noise tokens.** `digital`, `digital-mobile`,
///     `digital-rip`, `digital-hd`, `webrip`, `webdl`, `mobile`.
///     Case-insensitive, whole-word only. Not strictly required for
///     parse correctness (the parser tolerates trailing junk via
///     `.*?\.cbz$`) but cleaner output reads better in logs and the
///     dashboard.
///
///  5. **Wrap the rightmost in-range bare year as `(YYYY)`.** Scene
///     format places the release year last, after the issue number,
///     so rightmost = release year. Wrapping only the rightmost match
///     preserves title-internal years (`2000 AD`).
///
///  6. **Collapse whitespace runs** left by the strip steps.
///
///  7. **Append `.cbz`** so the trailing `\.(?i:cbz|cbr|cb7)$` anchor
///     in every pattern matches. The fact that scene NZBs aren't
///     actually CBZs is irrelevant — we're feeding the *parser*, not
///     the downloader.
pub fn normalize_scene_title(input: &str) -> String {
    let degrouped = strip_scene_group_suffix(input);
    let with_spaces = degrouped.replace('.', " ");
    let with_of = wrap_of_marker(&with_spaces);
    let denoised = strip_scene_noise(&with_of);
    let with_year = wrap_rightmost_year(&denoised);
    let collapsed: String = with_year.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{collapsed}.cbz")
}

/// Strip the trailing `-Group` scene suffix when present. The hyphen
/// must be the LAST one in the string and followed only by an
/// alphanumeric token to end-of-string — so `digital-mobile` mid-string
/// and `Spider-Man` series titles survive; only `-Empire`, `-DCP`,
/// `-LeDuch`, etc. at the very end get stripped.
fn strip_scene_group_suffix(input: &str) -> &str {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"-[A-Za-z][A-Za-z0-9]*$").unwrap());
    match re.find(input) {
        Some(m) => &input[..m.start()],
        None => input,
    }
}

/// Wrap `\d+ of \d+` mini-series markers as `\d+ (of \d+)`. Whole-word
/// digit-of-digit match, case-insensitive on the literal `of`. Letter-
/// flanked `of` (`Life of Wolverine`, `Eye of Odin`) is preserved
/// because the regex requires digits on both sides.
fn wrap_of_marker(input: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\b(\d+)\s+(?i:of)\s+(\d+)\b").unwrap());
    re.replace_all(input, "$1 (of $2)").into_owned()
}

/// Strip standalone scene-noise tokens. Whole-word case-insensitive
/// match; the order in the alternation matters only for readability.
/// `digital-mobile` and friends are listed BEFORE `digital` so the
/// longer match wins — the regex engine is greedy on alternation
/// position, not match length, so we put long-before-short by hand.
fn strip_scene_noise(input: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:digital-mobile|digital-rip|digital-hd|digital|webrip|webdl|mobile)\b",
        )
        .unwrap()
    });
    re.replace_all(input, " ").into_owned()
}

/// Wrap the rightmost in-range bare year token in parens. Skips
/// already-wrapped `(YYYY)` so a prior pass doesn't get double-wrapped.
fn wrap_rightmost_year(input: &str) -> String {
    let re = scene_year_re();
    let Some(last) = re.find_iter(input).last() else {
        return input.to_owned();
    };
    let bytes = input.as_bytes();
    // Don't double-wrap if the match is already inside `(...)`.
    let prev = last.start().checked_sub(1).and_then(|i| bytes.get(i)).copied();
    let next = bytes.get(last.end()).copied();
    if prev == Some(b'(') && next == Some(b')') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len() + 2);
    out.push_str(&input[..last.start()]);
    out.push('(');
    out.push_str(&input[last.start()..last.end()]);
    out.push(')');
    out.push_str(&input[last.end()..]);
    out
}

/// Run the parser on the raw title; fall back through two repair
/// passes when the raw parse fails. Cascade order is correctness-
/// critical:
///
/// 1. **Raw** — canonical filename input (`Wolverine 005 (2024).cbz`)
///    short-circuits everything below.
///
/// 2. **`{title}.cbz`** — NZBGeek (and any indexer with space-
///    separated, already-parenthesized titles) ships releases as
///    `Beneath the Trees Where Nobody Sees 001 (2023) (digital) (Son
///    of Ultron-Empire)` — no extension, year already wrapped. The
///    only thing keeping parse_filename from accepting these is the
///    trailing `\.(?i:cbz|cbr|cb7)$` anchor; appending `.cbz`
///    satisfies the anchor without touching the year token. Trying
///    this step BEFORE [`normalize_scene_title`] is load-bearing:
///    the normalizer's `\b(YYYY)\b` regex matches inside an existing
///    `(2023)` (word boundary fires at the paren↔digit edge), and
///    re-wraps it as `((2023))` which the year-capture group rejects.
///
/// 3. **`normalize_scene_title`** — dotted Scene format
///    (`Hello.Darkness.001.2024.digital.Son.of.Ultron-Empire`) still
///    needs the dots→spaces + bare-year-wrap + `.cbz` transform.
///    Reached only when the title has no extension AND no
///    parenthesized year — the conditions the normalizer was
///    actually designed for.
fn parse_release_title(title: &str, patterns: &[ParsingPattern]) -> Option<ParsedFilename> {
    if let Some(parsed) = parse_filename(title, patterns) {
        return Some(parsed);
    }
    if let Some(parsed) = parse_filename(&format!("{title}.cbz"), patterns) {
        return Some(parsed);
    }
    // Bracketed-tag titles (NZBGeek, Prowlarr) — try BEFORE the
    // scene normalizer because the scene step's rightmost-year-wrap
    // would land the year inside the brackets and produce `[(YYYY)]`
    // which still doesn't parse. The NZB normalizer strips the
    // brackets, capturing the year first, before re-emitting the
    // canonical shape.
    if let Some(parsed) = parse_filename(&normalize_nzb_title(title), patterns) {
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
    exclusion_keywords: &[String],
    min_size_bytes: Option<i64>,
) -> FilterOutcome {
    // Pre-grab size floor. Drop releases whose reported size is below
    // the configured Phase B threshold so we never even submit them to
    // SAB. `map_or(true, ...)` is load-bearing: indexers that don't
    // emit `newznab:attr name="size"` leave `size_bytes = None`, and
    // those must pass through — Phase B's post-download check still
    // catches them as a second line of defense. This runs BEFORE the
    // exclusion-keyword + parse + similarity filters so undersized
    // releases don't bloat `total_results` and mislead the diagnostic.
    let releases: Vec<Release> = if let Some(floor) = min_size_bytes {
        releases
            .into_iter()
            .filter(|r| r.size_bytes.map_or(true, |s| s >= floor))
            .collect()
    } else {
        releases
    };

    let requested_normalized = normalize_title(requested_series_title);

    // Pre-filter excluded releases BEFORE counting total_results. An
    // excluded release must NOT contribute to total_results,
    // parseable_count, or best_similarity — those signals drive the
    // mismatch-vs-silent-fallthrough decision below, and treating an
    // excluded release as a mismatch input would surface spurious
    // "0 parseable / wrong series" diagnostics when the indexer
    // happened to return only digital-only formats.
    //
    // Normalization (lowercase + dots → spaces) matches both
    // `Infinity Comic` (NZBGeek-style space-separated) and
    // `Infinity.Comic` (Scene-style dotted) with the same single
    // human-readable keyword in the config.
    let lowered_keywords: Vec<String> = exclusion_keywords
        .iter()
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty())
        .collect();
    let releases: Vec<Release> = if lowered_keywords.is_empty() {
        releases
    } else {
        releases
            .into_iter()
            .filter(|r| {
                let normalized_title = r.title.to_lowercase().replace('.', " ");
                !lowered_keywords
                    .iter()
                    .any(|kw| normalized_title.contains(kw))
            })
            .collect()
    };
    let total_results = releases.len();

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

/// Score a single release title against a requested series title (0.0–1.0).
///
/// The per-result confidence Interactive Search shows: reuses the exact
/// parse → normalize → similarity that the pre-grab filter
/// ([`filter_by_series_title`]) uses, just surfaced per-release instead of
/// thresholded away. `None` when the title can't be parsed into a series
/// segment at all — the caller renders that as zero confidence. Issue
/// number and year are deliberately NOT part of the score (per the kickoff
/// decision: confidence = series-title similarity).
pub fn score_release_title(
    release_title: &str,
    requested_series_title: &str,
    patterns: &[ParsingPattern],
) -> Option<f64> {
    let parsed = parse_release_title(release_title, patterns)?;
    let requested_normalized = normalize_title(requested_series_title);
    let parsed_normalized = normalize_title(&parsed.series_title);
    Some(similarity(&requested_normalized, &parsed_normalized))
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
/// the pool is empty. `min_size_bytes = Some(floor)` filters out any
/// release whose `size_bytes` is reported AND below `floor`; releases
/// with `size_bytes = None` always pass (the indexer didn't tell us,
/// so we can't reject — Phase B catches them post-download).
pub fn select_best(mut releases: Vec<Release>, min_size_bytes: Option<i64>) -> Option<Release> {
    if let Some(floor) = min_size_bytes {
        releases.retain(|r| r.size_bytes.map_or(true, |s| s >= floor));
    }
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

    #[test]
    fn score_release_title_high_for_match_low_for_wrong_series() {
        let patterns = default_patterns();
        let good = score_release_title("Saga 012 (2012) (digital).cbz", "Saga", &patterns).unwrap();
        assert!(good > 0.9, "correct series should score high, got {good}");
        let bad = score_release_title("Batman 012 (2016).cbz", "Saga", &patterns).unwrap();
        assert!(bad < 0.5, "wrong series should score low, got {bad}");
        // Scene-format title (dotted, group suffix) still parses + scores.
        let scene =
            score_release_title("Saga.012.2012.digital.Zone-Empire", "Saga", &patterns).unwrap();
        assert!(scene > 0.9, "scene-format should score high, got {scene}");
    }

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
        assert_eq!(select_best(pool, None).unwrap().title, "Wolverine 005.cbz");
    }

    #[test]
    fn within_same_format_prefers_higher_grabs() {
        let pool = vec![
            release("a.cbz", Some(5)),
            release("b.cbz", Some(80)),
            release("c.cbz", Some(40)),
        ];
        assert_eq!(select_best(pool, None).unwrap().title, "b.cbz");
    }

    #[test]
    fn recency_breaks_grab_ties() {
        let mut older = release("old.cbz", Some(10));
        older.published = Some(datetime!(2024-01-01 0:00 UTC));
        let mut newer = release("new.cbz", Some(10));
        newer.published = Some(datetime!(2025-01-01 0:00 UTC));
        let pool = vec![older, newer];
        assert_eq!(select_best(pool, None).unwrap().title, "new.cbz");
    }

    #[test]
    fn cbr_still_selectable_when_no_cbz() {
        let pool = vec![release("only.cbr", Some(1))];
        assert_eq!(select_best(pool, None).unwrap().title, "only.cbr");
    }

    #[test]
    fn empty_pool_is_none() {
        assert!(select_best(vec![], None).is_none());
    }

    // -------- size-floor pre-grab gate --------

    #[test]
    fn select_best_drops_release_below_size_floor() {
        let mut tiny = release("tiny.cbz", Some(10));
        tiny.size_bytes = Some(5 * 1024 * 1024); // 5 MB — below floor
        let mut ok = release("ok.cbz", Some(10));
        ok.size_bytes = Some(30 * 1024 * 1024); // 30 MB — above floor
        let pool = vec![tiny, ok];
        let pick = select_best(pool, Some(10 * 1024 * 1024)).unwrap();
        assert_eq!(pick.title, "ok.cbz");
    }

    #[test]
    fn select_best_passes_release_with_unknown_size_through_the_floor() {
        // `map_or(true, ...)` semantics: an indexer that doesn't emit
        // `<newznab:attr name="size">` leaves size_bytes = None. Those
        // releases must NOT be rejected — Phase B's post-download check
        // is the second line of defense for unknown-size cases.
        let unknown = release("unknown.cbz", Some(10)); // size_bytes = None
        let pool = vec![unknown];
        assert!(select_best(pool, Some(10 * 1024 * 1024)).is_some());
    }

    #[test]
    fn filter_by_series_title_drops_undersized_before_counting() {
        // Pre-grab size floor runs BEFORE the parse / similarity gates
        // so undersized releases don't bloat `total_results` and emit
        // misleading diagnostics. With one undersized release and one
        // OK release, the surviving outcome should reflect only the
        // OK release.
        let patterns = default_patterns();
        let mut undersized = release("Saga 001 (2012) (digital).cbz", Some(20));
        undersized.size_bytes = Some(2 * 1024 * 1024); // 2 MB — drop
        let mut healthy = release("Saga 002 (2012) (digital).cbz", Some(20));
        healthy.size_bytes = Some(40 * 1024 * 1024); // 40 MB — keep
        let pool = vec![undersized, healthy];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Saga",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &[],
            Some(10 * 1024 * 1024),
        );
        assert_eq!(outcome.kept.len(), 1);
        assert_eq!(outcome.kept[0].title, "Saga 002 (2012) (digital).cbz");
        assert!(outcome.mismatch.is_none());
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
        );
        assert!(outcome.kept.is_empty());
        assert!(
            outcome.mismatch.is_none(),
            "year-rejected (a) proves indexer has right series — must stay silent. got {:?}",
            outcome.mismatch
        );
    }

    // -------- exclusion_keywords (digital-only formats) --------

    /// The user's configured exclusion list keeps Marvel "Infinity
    /// Comic" releases out of the library. A release whose title
    /// contains the keyword must be silently dropped — NOT counted
    /// as a parseable mismatch (which would taint the
    /// best_similarity signal and surface a spurious mismatch row).
    #[test]
    fn exclusion_keyword_drops_infinity_comic_release() {
        let patterns = default_patterns();
        let pool = vec![release(
            "Amazing Spider-Man Infinity Comic 001 (2023) (digital).cbz",
            Some(40),
        )];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Amazing Spider-Man",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &["Infinity Comic".into()],
            None,
        );
        assert!(outcome.kept.is_empty(), "excluded release must be dropped");
        assert!(
            outcome.mismatch.is_none(),
            "exclusion is silent — not a series-mismatch. got {:?}",
            outcome.mismatch
        );
    }

    /// The match is normalized: lowercase + dots → spaces, so both
    /// upper-cased and Scene-format dotted spellings are caught with
    /// the same `Infinity Comic` keyword. This is the contract that
    /// keeps the user's config simple — one human-readable keyword,
    /// many on-the-wire shapes covered.
    #[test]
    fn exclusion_keyword_case_insensitive_and_dot_form() {
        let patterns = default_patterns();
        let pool = vec![
            release(
                "Amazing Spider-Man INFINITY COMIC 001 (2023) (digital).cbz",
                Some(40),
            ),
            release(
                "Amazing.Spider-Man.Infinity.Comic.002.2023.digital.Empire",
                Some(40),
            ),
        ];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Amazing Spider-Man",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &["Infinity Comic".into()],
            None,
        );
        assert!(
            outcome.kept.is_empty(),
            "both case-variant and dot-form must be caught"
        );
        assert!(outcome.mismatch.is_none());
    }

    /// Empty exclusion list is the production default before the
    /// settings migration runs and a hand-edited DB shape. Must
    /// behave as if the feature is off — a release that would
    /// otherwise pass the series + year gates passes through.
    #[test]
    fn exclusion_empty_list_passes_everything() {
        let patterns = default_patterns();
        let pool = vec![release(
            "Amazing Spider-Man 001 (2023) (digital).cbz",
            Some(40),
        )];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Amazing Spider-Man",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &[],
            None,
        );
        assert_eq!(
            outcome.kept.len(),
            1,
            "empty exclusions must not filter anything"
        );
    }

    /// The 20260602010000 migration extends the default keyword set
    /// with `Trade Paperback`. NZBs for TPB reissues of subscribed
    /// series must be silently dropped — they're the pull-side
    /// complement to the cv-enrichment collection penalty (which
    /// keeps the catalog row pointed at the original series).
    #[test]
    fn exclusion_drops_trade_paperback() {
        let patterns = default_patterns();
        let pool = vec![release(
            "Beneath the Trees Where Nobody Sees Trade Paperback (2024).cbz",
            Some(40),
        )];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Beneath the Trees Where Nobody Sees",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &["Trade Paperback".into()],
            None,
        );
        assert!(outcome.kept.is_empty());
        assert!(outcome.mismatch.is_none(), "exclusion must be silent");
    }

    /// `Hardcover` in the release title — same default-keyword set.
    #[test]
    fn exclusion_drops_hardcover() {
        let patterns = default_patterns();
        let pool = vec![release("Batman Hardcover 001 (2023).cbz", Some(40))];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Batman",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &["Hardcover".into()],
            None,
        );
        assert!(outcome.kept.is_empty());
        assert!(outcome.mismatch.is_none());
    }

    /// `Omnibus` in the release title.
    #[test]
    fn exclusion_drops_omnibus() {
        let patterns = default_patterns();
        let pool = vec![release("Spider-Man Omnibus 001 (2023).cbz", Some(40))];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Spider-Man",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &["Omnibus".into()],
            None,
        );
        assert!(outcome.kept.is_empty());
        assert!(outcome.mismatch.is_none());
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

    // -------- normalize_scene_title: dot-separated mini-series notation --------
    //
    // The third NZB title format Jeremy reported: fully dot-separated
    // scene names with `\d+.of.\d+` mini-series notation, no brackets.
    // Pre-fix, `normalize_scene_title` produced `01 of 04 (2025)` which
    // mis-captured through pattern id=10 as series=`... 01 of`,
    // number=`04`. The wrap_of_marker step now produces `01 (of 04)
    // (2025)` so pattern id=11 claims it cleanly.

    #[test]
    fn normalize_real_arcana_royale_of_marker_with_year() {
        // The exact user-reported title. End-to-end through the
        // parse_release_title cascade: scene normalizer + parser
        // pattern id=11.
        check_normalized_parses_to(
            "Arcana.Royale.01.of.04.2025.digital.Son.of.Ultron-Empire",
            "Arcana Royale",
            "01",
            Some(2025),
        );
    }

    #[test]
    fn normalize_dotted_of_marker_year_less_falls_to_id_13() {
        // Year-less variant: rightmost-year-wrap finds nothing, and
        // the `\d+ (of \d+)` shape falls through to pattern id=13
        // (`Series N (of M) yearless`).
        check_normalized_parses_to(
            "Some.Mini.02.of.04.digital.Empire",
            "Some Mini",
            "02",
            None,
        );
    }

    #[test]
    fn normalize_dotted_padded_of_marker() {
        // Three-digit issue number with two-digit mini count.
        // Confirms `\b(\d+)\s+(?i:of)\s+(\d+)\b` accepts either width
        // and the parser's `\(of\s+\d+\)` is similarly permissive.
        check_normalized_parses_to(
            "Be.Not.Afraid.004.of.006.2025.Digital-HD.LeDuch",
            "Be Not Afraid",
            "004",
            Some(2025),
        );
    }

    #[test]
    fn normalize_scene_letter_flanked_of_is_not_a_marker() {
        // `Life of Wolverine` has `of` flanked by letter tokens, NOT
        // by digit tokens. The wrap_of_marker regex must not fire
        // here or it'd produce `Life (of Wolverine)` and break the
        // existing real-world parse. This is the load-bearing guard
        // against false positives on titles containing the word `of`.
        check_normalized_parses_to(
            "Life.of.Wolverine.-.Infinity.Comic.005.2022.digital-mobile.Empire",
            "Life of Wolverine - Infinity Comic",
            "005",
            Some(2022),
        );
    }

    #[test]
    fn normalize_scene_eye_of_odin_still_works() {
        // Another letter-flanked `of` regression guard. This case
        // existed before; the new wrap_of_marker MUST leave it alone.
        check_normalized_parses_to(
            "Beware.the.Eye.of.Odin.001.2022.Digital.Mephisto-Empire",
            "Beware the Eye of Odin",
            "001",
            Some(2022),
        );
    }

    #[test]
    fn strip_scene_group_suffix_removes_trailing_dash_group_only() {
        // The helper removes a trailing `-Word` suffix but leaves
        // internal hyphenated tokens (`digital-mobile`, `Spider-Man`)
        // alone.
        assert_eq!(
            strip_scene_group_suffix("Foo.001.2022.Digital.Zone-Empire"),
            "Foo.001.2022.Digital.Zone"
        );
        assert_eq!(
            strip_scene_group_suffix("Spider-Man.005.2022.Digital.Empire"),
            "Spider-Man.005.2022.Digital.Empire",
            "Empire has no leading hyphen at the end — input unchanged"
        );
        assert_eq!(
            strip_scene_group_suffix("Foo.001.2022.digital-mobile.Empire"),
            "Foo.001.2022.digital-mobile.Empire",
            "Empire has no leading hyphen; digital-mobile mid-string is preserved"
        );
        assert_eq!(
            strip_scene_group_suffix("Foo.001.2022.digital.Hourman-DCP"),
            "Foo.001.2022.digital.Hourman"
        );
    }

    #[test]
    fn wrap_of_marker_handles_digit_flanked_only() {
        assert_eq!(wrap_of_marker("01 of 04"), "01 (of 04)");
        assert_eq!(wrap_of_marker("001 of 06"), "001 (of 06)");
        assert_eq!(wrap_of_marker("a 01 of 04 b"), "a 01 (of 04) b");
        // Letter-flanked `of` is preserved.
        assert_eq!(wrap_of_marker("Life of Wolverine"), "Life of Wolverine");
        assert_eq!(wrap_of_marker("Eye of Odin"), "Eye of Odin");
        // Multiple matches all get wrapped.
        assert_eq!(
            wrap_of_marker("01 of 04 and 02 of 05"),
            "01 (of 04) and 02 (of 05)"
        );
    }

    #[test]
    fn strip_scene_noise_drops_known_tokens_only() {
        // Whole-word, case-insensitive. Adjacent tokens get replaced
        // with a space; whitespace collapse happens at the call-site.
        assert_eq!(
            strip_scene_noise("Foo 001 2022 digital Empire"),
            "Foo 001 2022   Empire"
        );
        assert_eq!(
            strip_scene_noise("Foo 001 2022 Digital-Mobile bar"),
            "Foo 001 2022   bar"
        );
        // Letters embedded in larger words must not match (no false
        // positive on `digital-something-else` that we don't know).
        assert_eq!(
            strip_scene_noise("Foo digitalize bar"),
            "Foo digitalize bar",
            "`digitalize` isn't a noise token; substring match must not fire"
        );
    }

    // -------- NZB bracketed-tag normalizer (Tier 4 critical) --------

    /// The user-surfaced bug: NZBGeek-style bracketed titles never
    /// parsed and the indexer pool came back "none parseable" even
    /// when real releases existed. Both example titles from the
    /// kickoff brief must now claim through the new normalizer.
    #[test]
    fn nzb_bracketed_year_and_tags_space_separated_series() {
        check_normalized_parses_to(
            "Absolute Green Lantern 007 [2025] [digital-mobile] [Son of Ultron-Empire]",
            "Absolute Green Lantern",
            "007",
            Some(2025),
        );
    }

    #[test]
    fn nzb_bracketed_year_and_tags_dot_separated_series() {
        check_normalized_parses_to(
            "Absolute.Green.Lantern.008 [2026] [Webrip] [The Last Kryptonian-DCP]",
            "Absolute Green Lantern",
            "008",
            Some(2026),
        );
    }

    /// `[YYYY]` capture must be the *first* 4-digit bracket, ignoring
    /// non-year brackets that happen to precede it. A `[Digital]`
    /// tag in front of the `[2025]` year shouldn't poison the
    /// capture.
    #[test]
    fn nzb_year_capture_skips_non_numeric_brackets_in_front() {
        check_normalized_parses_to(
            "Some Mini 003 [Digital] [2025] [Group]",
            "Some Mini",
            "003",
            Some(2025),
        );
    }

    /// `[2025-Backup]` looks like it carries a year but the regex
    /// requires the whole bracket to be exactly four digits — so this
    /// is NOT a year capture, and the title falls through year-less.
    /// The `(of N) yearless` / catch-all patterns then pick it up,
    /// or it stays unparseable. Either way the year token is `None`.
    #[test]
    fn nzb_non_pure_year_bracket_does_not_capture_year() {
        let patterns = default_patterns();
        let parsed = parse_release_title(
            "Saga 042 [2025-Backup] [Digital] [Group]",
            &patterns,
        );
        // Year-less with the (of-N-style) catch-all may or may not
        // claim depending on shape; what's load-bearing is that we
        // don't accidentally capture `2025` from a non-pure bracket.
        if let Some(p) = parsed {
            assert_eq!(p.year, None, "year must NOT capture from non-pure bracket");
        }
    }

    /// Bracketed title without ANY [YYYY] bracket still strips the
    /// other brackets and tries to parse year-less. Whether it parses
    /// depends on whether the cleaned form matches a yearless pattern;
    /// the load-bearing assertion is the normalizer doesn't crash and
    /// doesn't fabricate a year.
    #[test]
    fn nzb_no_year_bracket_strips_other_tags_and_yields_yearless() {
        let normalized = normalize_nzb_title("Some Mini 003 [Digital] [Group-Name]");
        assert_eq!(normalized, "Some Mini 003.cbz");
    }

    /// Direct normalizer round-trip — the canonical-shape output the
    /// filename parser expects to receive.
    #[test]
    fn nzb_normalizer_emits_canonical_shape() {
        assert_eq!(
            normalize_nzb_title(
                "Absolute Green Lantern 007 [2025] [digital-mobile] [Son of Ultron-Empire]"
            ),
            "Absolute Green Lantern 007 (2025).cbz",
        );
        assert_eq!(
            normalize_nzb_title(
                "Absolute.Green.Lantern.008 [2026] [Webrip] [The Last Kryptonian-DCP]"
            ),
            "Absolute Green Lantern 008 (2026).cbz",
        );
    }

    /// The full select-stage path: a pool of bracketed-tag releases
    /// for a subscribed series must SURVIVE the filter (parseable,
    /// title-matched, year-matched). Pre-fix this returned an
    /// "all unparseable" mismatch; post-fix the pool is non-empty.
    #[test]
    fn filter_keeps_bracketed_releases_for_matching_series() {
        let patterns = default_patterns();
        let pool = vec![
            release(
                "Absolute Green Lantern 007 [2025] [digital-mobile] [Son of Ultron-Empire]",
                Some(20),
            ),
            release(
                "Absolute.Green.Lantern.008 [2026] [Webrip] [The Last Kryptonian-DCP]",
                Some(15),
            ),
        ];
        let outcome = filter_by_series_title(
            pool,
            &patterns,
            "Absolute Green Lantern",
            None,
            longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
            &[],
            None,
        );
        assert_eq!(
            outcome.kept.len(),
            2,
            "both bracketed releases must survive the filter; got {} (mismatch={:?})",
            outcome.kept.len(),
            outcome.mismatch
        );
    }

    /// **NZBGeek shape** — the format the indexer actually returns:
    /// space-separated, year already parenthesized, ANNOTATIONS in
    /// trailing parens, no extension. Pre-fix this fell through to
    /// `normalize_scene_title` and double-wrapped the year (the
    /// rightmost-year regex matches inside an existing `(2023)`
    /// because `\b` fires at the paren↔digit edge), producing
    /// `((2023))` and an unparseable input. Post-fix the
    /// `{title}.cbz` middle pass satisfies the extension anchor
    /// without touching the year token.
    #[test]
    fn parse_nzbgeek_space_separated_with_annotations() {
        check_normalized_parses_to(
            "Beneath the Trees Where Nobody Sees 001 (2023) (digital) (Son of Ultron-Empire)",
            "Beneath the Trees Where Nobody Sees",
            "001",
            Some(2023),
        );
    }

    /// **Regression** — any title that already has a parenthesized
    /// year segment must NOT have it double-wrapped on the way
    /// through `parse_release_title`. The contract: an
    /// already-(YYYY)-bearing input either parses raw (canonical
    /// `.cbz` shape) or via the `.cbz`-append middle pass — never
    /// via `normalize_scene_title`, whose year-wrap step is unsafe
    /// for already-wrapped years.
    ///
    /// Two minimal shapes cover the surface:
    ///   - extension-less, single annotation block
    ///   - extension-less, no trailing annotations at all
    #[test]
    fn double_wrap_does_not_occur_on_already_parenthesized_year() {
        // With trailing annotation.
        check_normalized_parses_to(
            "Saga 062 (2024) (digital)",
            "Saga",
            "062",
            Some(2024),
        );
        // Bare — year is the only parenthesized segment, no
        // annotation block at all. Pre-fix this was the cleanest
        // double-wrap reproducer.
        check_normalized_parses_to("Saga 062 (2024)", "Saga", "062", Some(2024));
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
            &[],
            None,
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
            &[],
            None,
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
            &[],
            None,
        );
        assert!(outcome.kept.is_empty());
        assert!(outcome.mismatch.is_none());
    }
}
