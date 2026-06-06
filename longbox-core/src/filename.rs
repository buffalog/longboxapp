//! Pure filename pattern matching. Takes the basename of a comic file and a
//! slice of [`ParsingPattern`]s (typically loaded from the `parsing_patterns`
//! table) and returns the first successful capture, ordered by `priority`
//! ascending. Patterns with `enabled = false` are skipped. Patterns that fail
//! to compile are skipped silently — the caller has more context for logging.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// A user-editable filename pattern. Stored in the `parsing_patterns` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsingPattern {
    pub id: i64,
    pub name: String,
    /// Named-capture regex. Required groups: `series`, `number`. Optional:
    /// `volume`, `year`, `title`.
    pub pattern: String,
    /// Lower value = tried first.
    pub priority: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedFilename {
    pub series_title: String,
    pub number: String,
    pub volume: Option<i32>,
    pub year: Option<i32>,
    pub title: Option<String>,
    /// ID of the pattern that matched. Lets the caller debug or surface
    /// "matched by pattern N" in the UI.
    pub pattern_id: i64,
}

pub fn parse(filename: &str, patterns: &[ParsingPattern]) -> Option<ParsedFilename> {
    let mut ordered: Vec<&ParsingPattern> = patterns.iter().filter(|p| p.enabled).collect();
    ordered.sort_by_key(|p| p.priority);

    for p in ordered {
        let Ok(re) = Regex::new(&p.pattern) else {
            continue;
        };
        let Some(caps) = re.captures(filename) else {
            continue;
        };
        let (Some(series), Some(number)) = (caps.name("series"), caps.name("number")) else {
            continue;
        };
        return Some(ParsedFilename {
            series_title: series.as_str().trim().to_owned(),
            number: number.as_str().trim().to_owned(),
            volume: caps
                .name("volume")
                .and_then(|m| m.as_str().parse::<i32>().ok()),
            year: caps
                .name("year")
                .and_then(|m| m.as_str().parse::<i32>().ok()),
            title: caps.name("title").map(|m| m.as_str().trim().to_owned()),
            pattern_id: p.id,
        });
    }
    None
}

/// Run [`parse`] on the basename; if no pattern claims it, retry with
/// [`normalize_dotted_basename`] applied first. The normalization
/// turns scene/NZB-style dot-separated names like
/// `Absolute.Green.Lantern.007.(2025).(Digital).(Shan-Empire).cbz`
/// into the canonical `Absolute Green Lantern 007 (2025).cbz` shape
/// the strict patterns already understand.
///
/// Cascade order is correctness-critical: original input wins on any
/// pattern hit. Without the original-first guard, the normalizer
/// would alter canonical inputs (e.g. it'd strip the optional year
/// from `Saga 001 (2014) - Volume One.cbz` and re-emit
/// `Saga 001 (2014).cbz`, losing the subtitle capture).
pub fn parse_with_normalization(
    basename: &str,
    patterns: &[ParsingPattern],
) -> Option<ParsedFilename> {
    if let Some(parsed) = parse(basename, patterns) {
        return Some(parsed);
    }
    let normalized = normalize_dotted_basename(basename);
    if normalized == basename {
        return None;
    }
    parse(&normalized, patterns)
}

/// Normalize a dot-separated filename (scene-release / NZB-style) into
/// the space-separated, parenthesized-year canonical shape the parsing
/// patterns expect. Mirrors `longbox_newznab::normalize_nzb_title` but
/// for filesystem basenames — extension is preserved, parens are the
/// metadata-tag separator (vs square brackets in the NZB form), and
/// year is captured from the FIRST `(YYYY)` in the stem.
///
/// Algorithm:
/// 1. Split the extension off (`.cbz` / `.cbr` / `.cb7`) so its dot
///    isn't replaced. Anything without a `.` returns unchanged.
/// 2. Capture the first `(YYYY)` from the stem — the release-year
///    tag scene releases (almost) always carry.
/// 3. Replace every `(...)` and `[...]` segment with a single space.
///    Erases scanlator markers, group tags, and the year we just
///    captured (we re-emit it canonically at the end).
/// 4. Replace dots with spaces — turns scene's separator into the
///    canonical patterns' separator.
/// 5. Collapse whitespace runs.
/// 6. Re-emit: `{cleaned} ({year}).{ext}` when year was captured,
///    `{cleaned}.{ext}` otherwise — the captured year goes back in
///    parens so the strict-year patterns (ids 2, 3, 7, 10, etc.) can
///    claim it.
///
/// Idempotent on its own output: a canonical filename (no dots in the
/// stem) round-trips unchanged after the first call.
pub fn normalize_dotted_basename(basename: &str) -> String {
    let (stem, ext) = match basename.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s, Some(e)),
        _ => return basename.to_owned(),
    };
    let year = dotted_year_re()
        .captures(stem)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_owned()));
    let without_parens = dotted_paren_re().replace_all(stem, " ");
    let without_brackets = dotted_bracket_re().replace_all(&without_parens, " ");
    let dotted = without_brackets.replace('.', " ");
    let cleaned: String = dotted.split_whitespace().collect::<Vec<_>>().join(" ");
    match (year, ext) {
        (Some(y), Some(e)) => format!("{cleaned} ({y}).{e}"),
        (None, Some(e)) => format!("{cleaned}.{e}"),
        // Both Some(ext)=None arms above cover every reachable
        // branch — `rsplit_once` returning None or Some with empty
        // sides already short-circuited above.
        _ => unreachable!("ext presence is decided at the rsplit_once branch"),
    }
}

static DOTTED_YEAR_RE: OnceLock<Regex> = OnceLock::new();
fn dotted_year_re() -> &'static Regex {
    DOTTED_YEAR_RE.get_or_init(|| Regex::new(r"\((\d{4})\)").unwrap())
}

static DOTTED_PAREN_RE: OnceLock<Regex> = OnceLock::new();
fn dotted_paren_re() -> &'static Regex {
    DOTTED_PAREN_RE.get_or_init(|| Regex::new(r"\([^)]*\)").unwrap())
}

static DOTTED_BRACKET_RE: OnceLock<Regex> = OnceLock::new();
fn dotted_bracket_re() -> &'static Regex {
    DOTTED_BRACKET_RE.get_or_init(|| Regex::new(r"\[[^\]]*\]").unwrap())
}

/// Default seed patterns. The DB migration in `longbox-db` will INSERT
/// matching rows; this helper keeps the patterns available for in-process
/// testing without depending on the DB layer.
///
/// Priority order — lower runs first. Patterns are listed below in
/// migration-insertion order (= id order): the original four (ids 1–4,
/// priorities 5/10/20/30), then the three added by the A.9 parser
/// hot-fix (ids 5–7, priorities 11/12/15). Keep this list in lockstep
/// with `longbox-db/migrations/*_parser_patterns.sql`.
pub fn default_patterns() -> Vec<ParsingPattern> {
    vec![
        ParsingPattern {
            id: 1,
            name: "Series Vol N #M".into(),
            pattern: r"^(?P<series>.+?)\s+(?i:v|vol|volume)\s*(?P<volume>\d+)\s+#?(?P<number>\d+(?:\.\d+)?).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 5,
            enabled: true,
        },
        ParsingPattern {
            id: 2,
            name: "Series #NNN (YYYY)".into(),
            pattern: r"^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})\)(?:\s+-\s+(?P<title>.+))?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 10,
            enabled: true,
        },
        ParsingPattern {
            id: 3,
            name: "Series NNN (YYYY)".into(),
            pattern: r"^(?P<series>.+?)\s+(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})\)\.(?i:cbz|cbr|cb7)$".into(),
            priority: 20,
            enabled: true,
        },
        ParsingPattern {
            id: 4,
            name: "Series_NNN or Series NNN".into(),
            pattern: r"^(?P<series>.+?)[_\s]+#?(?P<number>\d+(?:\.\d+)?)\.(?i:cbz|cbr|cb7)$".into(),
            priority: 30,
            enabled: true,
        },
        // A.9 parser hot-fix — three shapes the original four miss.
        // Each carries a permissive `.*?\.<ext>$` tail so trailing
        // scanlator markers like `(Digital) (Mephisto-Empire)` don't
        // block the match.
        ParsingPattern {
            id: 5,
            name: "Series N (Xf Y) (YYYY)".into(),
            // Part-of-N marker between number and year (20th Century
            // Men 01 (0f 06) (2022).cbr). Conservative literal `Xf Y`
            // — broader paren-tolerance is a separate fix.
            pattern: r"^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(\d+f\s*\d+\)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 11,
            enabled: true,
        },
        ParsingPattern {
            id: 6,
            name: "Series N - Subtitle (YYYY)".into(),
            // Subtitle between number and year (Aama 01 - The Smell
            // of Warm Dust (2013) (Digital).cbr). New pattern rather
            // than rewriting id=2 so id=2's existing tests stay
            // undisturbed.
            pattern: r"^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+-\s+(?P<title>.+?)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 12,
            enabled: true,
        },
        ParsingPattern {
            id: 7,
            name: "Series (YYYY) NNN".into(),
            // Year-in-parens BEFORE the number — the user's standard
            // library convention (Wolverine (2024) 001.cbz). The
            // load-bearing fix; covers ~75% of pre-hot-fix unmatched
            // files. Without it, the catch-all (id=4, priority 30)
            // absorbs these but bakes the year into `series_title`,
            // which poisons the scanner's title-similarity match.
            pattern: r"^(?P<series>.+?)\s+\((?P<year>\d{4})\)\s+#?(?P<number>\d+(?:\.\d+)?).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 15,
            enabled: true,
        },
        // A.9 Bug 1b — three more shapes Bug 1a's rollback surfaced.
        // Specific markers (Book, vN-Subtitle) get low priority so
        // they claim before the strict patterns; the permissive
        // YYYY[-MM] catcher sits at priority 25 just above the
        // catch-all so it only fires on what the strict patterns miss.
        ParsingPattern {
            id: 8,
            name: "Series vN - Subtitle (YYYY)".into(),
            // TPB volume + subtitle: `Fear Agent v01 - Re-Ignition
            // (2007) (digital) (Son of Ultron-Empire).cbr`. Captures
            // the volume number AS the issue number — pragmatic for
            // TPB-only series. Hybrid series with both single-issue
            // #N and TPB vN would collide; see A.9 prompt's deferred
            // items.
            pattern: r"^(?P<series>.+?)\s+(?i:v|vol|volume)\s*(?P<number>\d+)\s+-\s+.+?\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 6,
            enabled: true,
        },
        ParsingPattern {
            id: 9,
            name: "Series Book N (YYYY)".into(),
            // Literal `Book N` marker: `Promethea Book 1 (1999)
            // (digital) (Tithe-Empire).cbr`. Strips "Book" from the
            // series capture so the catalog records "Promethea" not
            // "Promethea Book".
            pattern: r"^(?P<series>.+?)\s+(?i:book)\s+(?P<number>\d+)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 7,
            enabled: true,
        },
        ParsingPattern {
            id: 10,
            name: "Series NNN (YYYY[-MM]) permissive".into(),
            // Permissive tail + optional year-month stamp:
            // `The Authority Revolution 001 (2004-12) (DeadmanWade-DCP)
            // (digital).cbr`. Sits BELOW all strict-year patterns so
            // they win first — preserves id=2's title capture on
            // `Saga 001 (2014) - Volume One.cbz`. Title group uses
            // `[^()]+` so a trailing scanlator paren-group is not
            // consumed as title.
            pattern: r"^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})(?:-\d{1,2})?\)(?:\s+-\s+(?P<title>[^()]+))?.*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 25,
            enabled: true,
        },
        // `(of N)` part-of-mini marker — `A Vicious Circle 002 (of 03)
        // (2023) (Digital-Empire).cbr`. Three variants cover the
        // common shape combinations: with year directly after the
        // marker, year-first with marker after the number, and
        // year-less. Without these, every existing pattern fails the
        // load-bearing scan path (year-after-number patterns reject
        // the `(of NN)` interloper; the catchall has a strict end);
        // `parse_basename` returns None and the scanner errors at
        // 422.
        ParsingPattern {
            id: 11,
            name: "Series N (of M) (YYYY)".into(),
            pattern: r"^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(of\s+\d+\)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 13,
            enabled: true,
        },
        ParsingPattern {
            id: 12,
            name: "Series (YYYY) N (of M)".into(),
            // Year-first variant — `Wolverine (2024) 003 (of 06)
            // (Digital).cbr`-shaped filenames where the year sits
            // before the number and the part-of marker after. Mirrors
            // id=7's year-first ordering.
            pattern: r"^(?P<series>.+?)\s+\((?P<year>\d{4})\)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(of\s+\d+\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 14,
            enabled: true,
        },
        ParsingPattern {
            id: 13,
            name: "Series N (of M) yearless".into(),
            // No year anchor. Sits just above the catch-all so it only
            // fires on shapes the strict patterns (11, 12) and every
            // pattern above them couldn't claim. Permissive `.*?`
            // tolerates trailing scanlator markers without a year.
            pattern: r"^(?P<series>.+?)\s+#?(?P<number>\d+(?:\.\d+)?)\s+\(of\s+\d+\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 28,
            enabled: true,
        },
        ParsingPattern {
            id: 14,
            name: "Series vNN (YYYY) no subtitle".into(),
            // TPB / collection volume notation WITHOUT a subtitle
            // separator: `EC - Cruel Universe v01 (2025).cbz`,
            // `Hunt. Kill. Repeat. v01 (2023) (Collection).cbz`.
            // Mirrors id=8's `Series vN - Subtitle (YYYY)` shape but
            // with `(YYYY)` directly after the volume number; id=8
            // can't claim these because it requires ` - Subtitle ` in
            // between. Captures volume number AS the issue number
            // (same pragmatic choice id=8 makes for TPB-only series).
            //
            // Priority 9 — between id=9 (Book N at 7) and id=2 (#NNN
            // at 10), so the more specific volume form wins before the
            // bare-digit patterns get to backtrack-greedy-eat the
            // `vNN` token as part of the series capture. The series
            // group's non-greedy `.+?` happily absorbs title-internal
            // dots (`Hunt. Kill. Repeat.`) and hyphens (`EC -`)
            // because the trailing-period / trailing-dash isn't
            // ambiguous — the only `\s+(?i:v|vol|volume)\s*\d+` lookahead
            // sits at the boundary the series capture must end at.
            pattern: r"^(?P<series>.+?)\s+(?i:v|vol|volume)\s*(?P<number>\d+(?:\.\d+)?)\s+\((?P<year>\d{4})\).*?\.(?i:cbz|cbr|cb7)$".into(),
            priority: 9,
            enabled: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns() -> Vec<ParsingPattern> {
        default_patterns()
    }

    #[test]
    fn matches_series_hash_number_year_title() {
        let p = parse("Saga #1 (2012) - The Beginning.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Saga");
        assert_eq!(p.number, "1");
        assert_eq!(p.year, Some(2012));
        assert_eq!(p.title.as_deref(), Some("The Beginning"));
        // id=2 = "Series #NNN (YYYY)" at priority 10.
        assert_eq!(p.pattern_id, 2);
    }

    #[test]
    fn matches_series_number_year_no_title() {
        let p = parse("Saga 1 (2012).cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Saga");
        assert_eq!(p.number, "1");
        assert_eq!(p.year, Some(2012));
        assert!(p.title.is_none());
        // The #NNN (YYYY) pattern's `#?` makes it match "1 (2012)" too, so
        // it wins over the NNN (YYYY) pattern (id=3) at higher priority.
        assert_eq!(p.pattern_id, 2);
    }

    #[test]
    fn matches_series_underscore_number() {
        let p = parse("Saga_1.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Saga");
        assert_eq!(p.number, "1");
        assert!(p.year.is_none());
        // id=4 = "Series_NNN or Series NNN" at priority 30.
        assert_eq!(p.pattern_id, 4);
    }

    #[test]
    fn matches_series_space_number_no_year() {
        let p = parse("Saga 1.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Saga");
        assert_eq!(p.number, "1");
        assert_eq!(p.pattern_id, 4);
    }

    #[test]
    fn volume_form_matched_by_priority_5_pattern() {
        // The Step 4 reordering put the volume-aware pattern at priority 5
        // (case-insensitive `(?i:v|vol|volume)`), so it claims Vol-form
        // filenames before the looser patterns absorb them.
        let p = parse("Iron Man v1 #100.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Iron Man");
        assert_eq!(p.volume, Some(1));
        assert_eq!(p.number, "100");
        assert_eq!(p.pattern_id, 1);

        let p = parse("Iron Man volume 2 #1.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Iron Man");
        assert_eq!(p.volume, Some(2));
        assert_eq!(p.number, "1");
        assert_eq!(p.pattern_id, 1);

        // Case-insensitivity: capital Vol matches now.
        let p = parse("Iron Man Vol 1 #100.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Iron Man");
        assert_eq!(p.volume, Some(1));
        assert_eq!(p.pattern_id, 1);
    }

    #[test]
    fn volume_form_with_trailing_content_still_matched() {
        // Trailing content like " Annual" after the issue number is consumed
        // by the volume pattern's `.*?\.<ext>$` tail.
        let p = parse("Iron Man Vol 1 #100 Annual.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Iron Man");
        assert_eq!(p.volume, Some(1));
        assert_eq!(p.number, "100");
        assert_eq!(p.pattern_id, 1);
    }

    #[test]
    fn matches_decimal_issue_number() {
        let p = parse("Saga #1.5 (2012).cbz", &patterns()).unwrap();
        assert_eq!(p.number, "1.5");
        // #NNN (YYYY) pattern (id=2) wins.
        assert_eq!(p.pattern_id, 2);
    }

    #[test]
    fn matches_case_insensitive_extension() {
        let p = parse("Saga 1.CBZ", &patterns()).unwrap();
        assert_eq!(p.series_title, "Saga");
        assert_eq!(p.number, "1");
    }

    #[test]
    fn matches_cbr_and_cb7_extensions() {
        assert_eq!(
            parse("Saga 1.cbr", &patterns()).unwrap().series_title,
            "Saga"
        );
        assert_eq!(
            parse("Saga 1.cb7", &patterns()).unwrap().series_title,
            "Saga"
        );
    }

    #[test]
    fn no_match_on_junk_filename() {
        assert!(parse("README.txt", &patterns()).is_none());
        assert!(parse(".DS_Store", &patterns()).is_none());
        assert!(parse("cover.jpg", &patterns()).is_none());
    }

    #[test]
    fn no_match_when_extension_missing() {
        assert!(parse("Saga 1", &patterns()).is_none());
    }

    #[test]
    fn disabled_patterns_are_skipped() {
        let mut ps = patterns();
        for p in &mut ps {
            p.enabled = false;
        }
        assert!(parse("Saga 1 (2012).cbz", &ps).is_none());
    }

    #[test]
    fn priority_order_is_respected() {
        // Construct two patterns that both match. The lower-priority one wins.
        let ps = vec![
            ParsingPattern {
                id: 100,
                name: "high priority".into(),
                pattern: r"^(?P<series>[A-Z]+)_(?P<number>\d+)\.cbz$".into(),
                priority: 1,
                enabled: true,
            },
            ParsingPattern {
                id: 200,
                name: "low priority".into(),
                pattern: r"^(?P<series>.+?)_(?P<number>\d+)\.cbz$".into(),
                priority: 99,
                enabled: true,
            },
        ];
        let p = parse("ABC_1.cbz", &ps).unwrap();
        assert_eq!(p.pattern_id, 100);
    }

    #[test]
    fn invalid_regex_is_skipped() {
        let ps = vec![
            ParsingPattern {
                id: 1,
                name: "broken".into(),
                pattern: r"(((".into(),
                priority: 1,
                enabled: true,
            },
            ParsingPattern {
                id: 2,
                name: "ok".into(),
                pattern: r"^(?P<series>.+?)_(?P<number>\d+)\.cbz$".into(),
                priority: 2,
                enabled: true,
            },
        ];
        let p = parse("Saga_1.cbz", &ps).unwrap();
        assert_eq!(p.pattern_id, 2);
    }

    #[test]
    fn empty_pattern_list_returns_none() {
        assert!(parse("Saga 1.cbz", &[]).is_none());
    }

    #[test]
    fn extra_trailing_content_in_title() {
        let p = parse(
            "The Walking Dead #1 (2003) - Days Gone Bye.cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "The Walking Dead");
        assert_eq!(p.number, "1");
        assert_eq!(p.year, Some(2003));
        assert_eq!(p.title.as_deref(), Some("Days Gone Bye"));
    }

    // ---- A.9 parser hot-fix: three shapes the original four miss ----

    #[test]
    fn matches_series_year_first_then_number_id_7() {
        // Pattern id=7 (priority 15) — the load-bearing F4c fix. The
        // user's standard library convention: year-in-parens BEFORE
        // number. Without this pattern, the catch-all id=4 absorbed
        // these and baked the year into `series_title` (e.g.
        // "Wolverine (2024)"), which poisoned the scanner's
        // title-similarity match against CV's clean sort_title.
        let p = parse("Wolverine (2024) 001.cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Wolverine");
        assert_eq!(p.number, "001");
        assert_eq!(p.year, Some(2024));
        assert_eq!(p.pattern_id, 7);

        let p = parse("The Walking Dead Deluxe (2020) 152.cbr", &patterns()).unwrap();
        assert_eq!(p.series_title, "The Walking Dead Deluxe");
        assert_eq!(p.number, "152");
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.pattern_id, 7);

        // Trailing scanlator markers tolerated.
        let p = parse(
            "Daredevil (2023) 005 (Digital) (Zone-Empire).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Daredevil");
        assert_eq!(p.number, "005");
        assert_eq!(p.year, Some(2023));
        assert_eq!(p.pattern_id, 7);
    }

    #[test]
    fn matches_series_number_subtitle_year_id_6() {
        // Pattern id=6 (priority 12) — F4b. Subtitle between number
        // and year. New pattern rather than rewriting id=2 so the
        // existing id=2 tests stay undisturbed.
        let p = parse(
            "Aama 01 - The Smell of Warm Dust (2013) (Digital) (Dipole-Empire).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Aama");
        assert_eq!(p.number, "01");
        assert_eq!(p.year, Some(2013));
        assert_eq!(p.title.as_deref(), Some("The Smell of Warm Dust"));
        assert_eq!(p.pattern_id, 6);

        // With `#` prefix on number.
        let p = parse("Series #1 - A Title Here (2020).cbz", &patterns()).unwrap();
        assert_eq!(p.series_title, "Series");
        assert_eq!(p.number, "1");
        assert_eq!(p.title.as_deref(), Some("A Title Here"));
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.pattern_id, 6);
    }

    #[test]
    fn matches_series_number_part_marker_year_id_5() {
        // Pattern id=5 (priority 11) — F4a. Part-of-N marker
        // between number and year. Conservative literal `Xf Y`;
        // broader paren-tolerance is a separate fix.
        let p = parse("20th Century Men 01 (0f 06) (2022).cbr", &patterns()).unwrap();
        assert_eq!(p.series_title, "20th Century Men");
        assert_eq!(p.number, "01");
        assert_eq!(p.year, Some(2022));
        assert_eq!(p.pattern_id, 5);

        // Trailing scanlator markers tolerated.
        let p = parse(
            "20th Century Men 02 (0f 06) (2022) (Digital) (Mephisto-Empire).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "20th Century Men");
        assert_eq!(p.number, "02");
        assert_eq!(p.year, Some(2022));
        assert_eq!(p.pattern_id, 5);
    }

    #[test]
    fn new_patterns_do_not_steal_claims_from_the_originals() {
        // Regression bar: the three new patterns must not steal
        // claims that the original four (ids 1–4) already handle.
        // Every existing-test case stays on its original pattern_id.
        // A future pattern-list reorder that silently breaks claim
        // ordering surfaces here.

        // id=1 (priority 5) — Vol-form runs before every new pattern.
        let p = parse("Iron Man Vol 1 #100.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 1, "Vol-form still claimed by id=1");

        // id=2 (priority 10) — number-then-year. id=7 needs year
        // BEFORE number; id=6 needs ` - title `; neither triggers.
        let p = parse("Saga #1 (2012).cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 2, "number-then-year still claimed by id=2");

        let p = parse("Saga #1 (2012) - The Beginning.cbz", &patterns()).unwrap();
        assert_eq!(
            p.pattern_id, 2,
            "number-then-year with trailing title still claimed by id=2"
        );

        // id=4 (priority 30) — catch-all for no-year filenames.
        // No new pattern claims a year-less filename.
        let p = parse("Saga_1.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 4, "underscored no-year shape still claimed by id=4");

        let p = parse("Saga 1.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 4, "spaced no-year shape still claimed by id=4");
    }

    // ---- A.9 Bug 1b: three more shapes Bug 1a's rollback surfaced ----

    #[test]
    fn matches_series_vol_subtitle_year_id_8() {
        // Pattern id=8 (priority 6) — TPB volume + subtitle:
        // `Fear Agent v01 - Re-Ignition (2007) (digital) (Son of
        // Ultron-Empire).cbr`. Captures the volume number AS the
        // issue number — pragmatic for TPB-only series.
        let p = parse(
            "Fear Agent v01 - Re-Ignition (2007) (digital) (Son of Ultron-Empire).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Fear Agent");
        assert_eq!(p.number, "01");
        assert_eq!(p.year, Some(2007));
        assert_eq!(p.pattern_id, 8);

        // Uppercase `V` works (case-insensitive marker).
        let p = parse(
            "Fear Agent V04 - Hatchet Job (2008) (digital) (Minutemen-InnerDemons).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Fear Agent");
        assert_eq!(p.number, "04");
        assert_eq!(p.year, Some(2008));
        assert_eq!(p.pattern_id, 8);

        // Spelled-out "Volume" also works.
        let p = parse(
            "Ranma 1/2 Volume 3 - Some Subtitle (1995).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.number, "3");
        assert_eq!(p.year, Some(1995));
        assert_eq!(p.pattern_id, 8);
    }

    #[test]
    fn matches_series_book_number_year_id_9() {
        // Pattern id=9 (priority 7) — TPB Book N:
        // `Promethea Book 1 (1999) (digital) (Tithe-Empire).cbr`.
        // The literal "Book" marker is stripped from series_title so
        // the catalog records the series as "Promethea" not
        // "Promethea Book".
        let p = parse(
            "Promethea Book 1 (1999) (digital) (Tithe-Empire).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Promethea");
        assert_eq!(p.number, "1");
        assert_eq!(p.year, Some(1999));
        assert_eq!(p.pattern_id, 9);

        // No scanlator suffix still works.
        let p = parse("Promethea Book 5 (2003).cbr", &patterns()).unwrap();
        assert_eq!(p.series_title, "Promethea");
        assert_eq!(p.number, "5");
        assert_eq!(p.year, Some(2003));
        assert_eq!(p.pattern_id, 9);
    }

    #[test]
    fn matches_series_number_year_month_id_10() {
        // Pattern id=10 (priority 25) — permissive tail + optional
        // year-month stamp: `The Authority Revolution 001 (2004-12)
        // (DeadmanWade-DCP) (digital).cbr`. Year capture is the YYYY
        // portion only; the `-MM` suffix is consumed but discarded.
        let p = parse(
            "The Authority Revolution 001 (2004-12) (DeadmanWade-DCP) (digital).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "The Authority Revolution");
        assert_eq!(p.number, "001");
        assert_eq!(p.year, Some(2004));
        assert_eq!(p.pattern_id, 10);

        // Plain (YYYY) + trailing scanlator (no dash, no subtitle)
        // — id=2 strict ending fails, id=10 catches the fallthrough.
        let p = parse(
            "Some Series 042 (2023) (digital) (Empire).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Some Series");
        assert_eq!(p.number, "042");
        assert_eq!(p.year, Some(2023));
        assert_eq!(p.pattern_id, 10);

        // Subtitle case: a hypothetical `Title 001 (2024-01) - Side
        // Story (digital).cbz` would be caught here (id=2 fails on
        // the year-month stamp), and id=10's `[^()]+` title group
        // captures only the subtitle without the trailing
        // scanlator. Not a shape seen in the user's library yet but
        // worth locking the capture semantics.
        let p = parse(
            "Title 001 (2024-01) - Side Story (digital).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Title");
        assert_eq!(p.number, "001");
        assert_eq!(p.year, Some(2024));
        assert_eq!(p.title.as_deref(), Some("Side Story"));
        assert_eq!(p.pattern_id, 10);
    }

    #[test]
    fn new_patterns_do_not_steal_claims_from_originals_v2() {
        // The kickoff's load-bearing regression check: ids 8/9/10
        // must not steal claims the existing patterns handle. The
        // Saga test case is the most important — it confirms
        // id=10's permissive tail does not undercut id=2's title
        // capture on a plain `(YYYY) - Subtitle.cbz` shape.

        // id=2 (priority 10) wins on plain-year subtitle shape.
        // Without the priority-25 placement, id=10 would steal this
        // and lose the title capture (title regex uses `[^()]+`
        // which differs from id=2's greedy `.+`).
        let p = parse("Saga 001 (2014) - Volume One.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 2, "plain-year subtitle still claimed by id=2");
        assert_eq!(p.title.as_deref(), Some("Volume One"));

        // id=2 still wins on plain-year no-subtitle shape.
        let p = parse("Saga 001 (2014).cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 2, "plain-year no-subtitle still claimed by id=2");
        assert_eq!(p.year, Some(2014));

        // id=7 (year-first) still wins on Wolverine shape.
        let p = parse("Wolverine (2024) 001.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 7, "year-first still claimed by id=7");

        // id=5/6 (part-of-N, subtitle-between) still win on their
        // respective shapes — id=8 needs `v`/`vol` prefix, not
        // applicable here.
        let p = parse("Aama 01 - The Smell of Warm Dust (2013).cbr", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 6, "subtitle-between still claimed by id=6");

        // id=4 (catch-all) still wins on year-less filenames — none
        // of the new patterns claim a no-year shape.
        let p = parse("Saga 1.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 4, "spaced no-year still claimed by id=4");
    }

    /// `(of N)` part-of-mini marker — the load-bearing case the user
    /// surfaced via HTTP 422 on `A Vicious Circle 002 (of 03) (2023)
    /// (Digital-Empire).cbr`. Without ids 11/12/13 every other
    /// pattern fails: the year-after-number variants (2, 3, 10)
    /// can't tolerate the interloping marker; id=5's part-of marker
    /// is the different `(Xf Y)` shape; the catch-all (id=4) has a
    /// strict end. `parse_basename` returned None and the scanner
    /// errored.
    #[test]
    fn of_n_marker_with_year_is_claimed_by_id_11() {
        let p = parse(
            "A Vicious Circle 002 (of 03) (2023) (Digital-Empire).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.pattern_id, 11);
        assert_eq!(p.series_title, "A Vicious Circle");
        assert_eq!(p.number, "002");
        assert_eq!(p.year, Some(2023));
    }

    #[test]
    fn of_n_marker_with_single_digit_count() {
        // `(of 6)` rather than `(of 06)` — the regex's `\d+` matches
        // either, but worth a sanity check on the single-digit form.
        let p = parse("Mini 03 (of 6) (2020).cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 11);
        assert_eq!(p.series_title, "Mini");
        assert_eq!(p.number, "03");
        assert_eq!(p.year, Some(2020));
    }

    #[test]
    fn of_n_marker_year_first_is_claimed_by_id_12() {
        let p = parse(
            "Wolverine (2024) 003 (of 06) (Digital).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.pattern_id, 12);
        assert_eq!(p.series_title, "Wolverine");
        assert_eq!(p.number, "003");
        assert_eq!(p.year, Some(2024));
    }

    #[test]
    fn of_n_marker_yearless_is_claimed_by_id_13() {
        // No year anywhere — `Series N (of M).cbz` style. id=13 sits
        // above the catch-all (id=4) so it claims the `(of N)` shape
        // even when no year shows up.
        let p = parse("Some Mini 02 (of 04).cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 13);
        assert_eq!(p.series_title, "Some Mini");
        assert_eq!(p.number, "02");
        assert_eq!(p.year, None);
    }

    #[test]
    fn of_n_patterns_do_not_steal_claims_from_existing_shapes() {
        // The `(of N)` patterns must not poach filenames the existing
        // strict patterns handle. Spot-check the most common shapes
        // claim the right pattern.
        let p = parse("Saga 001 (2014).cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 2, "plain (YYYY) still id=2, not an (of N) variant");

        let p = parse("Wolverine (2024) 001.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 7, "plain year-first still id=7");

        let p = parse("20th Century Men 01 (0f 06) (2022).cbr", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 5, "(Xf Y) part-of still id=5, not the new (of N)");
    }

    // ---- id=14: `Series vNN (YYYY)` collected editions, no subtitle ----

    #[test]
    fn matches_series_vnn_year_no_subtitle_id_14() {
        // The exact user-reported filename: EC's `Cruel Universe v01`
        // TPB. Pre-fix this fell through every pattern and Phase B
        // routed it to /watch/ as Skipped. id=14 claims it now with
        // the volume number as the issue number.
        let p = parse(
            "EC - Cruel Universe v01 (2025) (digital) (Son of Ultron-Empire).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.pattern_id, 14);
        assert_eq!(p.series_title, "EC - Cruel Universe");
        assert_eq!(p.number, "01");
        assert_eq!(p.year, Some(2025));

        // Title-internal dots + volume notation:
        // `Hunt. Kill. Repeat. v01 (2023) (Collection) ...`.
        let p = parse(
            "Hunt. Kill. Repeat. v01 (2023) (Collection) (digital) (Son of Ultron-Empire).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.pattern_id, 14);
        assert_eq!(p.series_title, "Hunt. Kill. Repeat.");
        assert_eq!(p.number, "01");
        assert_eq!(p.year, Some(2023));

        // Case-insensitive `V` / `Vol` / `Volume` markers.
        let p = parse("Foo Vol 3 (1995).cbr", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 14);
        assert_eq!(p.number, "3");

        let p = parse("Foo Volume 12 (2020).cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 14);
        assert_eq!(p.number, "12");
    }

    #[test]
    fn id_14_does_not_steal_claims_from_id_8() {
        // id=8 (Series vN - Subtitle (YYYY)) is more specific (it
        // requires a subtitle); it MUST still win on its shape even
        // though id=14 would match too. Priority ordering: id=8 at 6
        // vs id=14 at 9.
        let p = parse(
            "Fear Agent v01 - Re-Ignition (2007) (digital) (Son of Ultron-Empire).cbr",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.pattern_id, 8, "vN with subtitle must still claim via id=8");
    }

    #[test]
    fn id_14_does_not_steal_claims_from_id_1() {
        // id=1 (Series Vol N #M) handles single-issue volume form
        // (`Iron Man Vol 1 #100`); id=14 requires `(YYYY)` after the
        // volume so it leaves these alone. Confirm id=1 still wins.
        let p = parse("Iron Man Vol 1 #100.cbz", &patterns()).unwrap();
        assert_eq!(p.pattern_id, 1, "Vol N #M still claimed by id=1");
        assert_eq!(p.volume, Some(1));
        assert_eq!(p.number, "100");
    }

    // ---- regression locks for filenames that already parse correctly ----
    //
    // The user reported these as Phase B failures along with the
    // vNN-format ones. Tracing showed they actually parse cleanly via
    // id=10's permissive tail; the Phase B failure must've been a
    // downstream issue (missing catalog series, etc.). Lock the
    // current correct behavior so a future pattern reorder can't
    // silently regress them.

    #[test]
    fn series_with_short_prefix_and_hyphen_parses_via_id_10() {
        // `Y - The Last Man` — single-letter prefix joined to the
        // series name by ` - `. id=10 backtracks past the hyphen and
        // captures the full series correctly.
        let p = parse(
            "Y - The Last Man 057 (2007) (Digital) (Monafekk-Empire).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.pattern_id, 10);
        assert_eq!(p.series_title, "Y - The Last Man");
        assert_eq!(p.number, "057");
        assert_eq!(p.year, Some(2007));
    }

    #[test]
    fn series_with_internal_dots_and_hyphen_parses_via_id_10() {
        // `DC K.O. - Superman vs. Captain Atom` — multiple
        // title-internal dots PLUS a ` - ` separator. id=10's
        // non-greedy series capture handles both.
        let p = parse(
            "DC K.O. - Superman vs. Captain Atom 001 (2026) (digital) (Son of Ultron-Empire).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.pattern_id, 10);
        assert_eq!(p.series_title, "DC K.O. - Superman vs. Captain Atom");
        assert_eq!(p.number, "001");
        assert_eq!(p.year, Some(2026));
    }

    // ---- dot-to-space normalization for scene/NZB-style filenames ----

    #[test]
    fn normalize_dotted_basename_user_example() {
        // The load-bearing case from the bug report. Without this
        // normalization Phase B sends the file to `_unsorted/`
        // because none of the strict patterns tolerate dot
        // separators between series tokens.
        let n = normalize_dotted_basename(
            "Absolute.Green.Lantern.007.(2025).(Digital).(Shan-Empire).cbz",
        );
        assert_eq!(n, "Absolute Green Lantern 007 (2025).cbz");
    }

    #[test]
    fn normalize_dotted_basename_handles_cbr_extension() {
        let n = normalize_dotted_basename(
            "Some.Series.001.(2024).(digital).(Group-Empire).cbr",
        );
        assert_eq!(n, "Some Series 001 (2024).cbr");
    }

    #[test]
    fn normalize_dotted_basename_without_year_tag() {
        // No `(YYYY)` to capture — re-emit year-less so the catch-all
        // (id=4) can still claim the result.
        let n = normalize_dotted_basename("Saga.001.cbz");
        assert_eq!(n, "Saga 001.cbz");
    }

    #[test]
    fn normalize_dotted_basename_strips_square_brackets_too() {
        // Square-bracket scanlator tags appear in some NZB-style
        // names; same disposition as parens.
        let n = normalize_dotted_basename(
            "Title.001.(2024).[digital].[Empire].cbz",
        );
        assert_eq!(n, "Title 001 (2024).cbz");
    }

    #[test]
    fn normalize_dotted_basename_first_year_wins() {
        // Multiple `(YYYY)` segments: first one wins (the release
        // year tag; later ones are typically scanlator group years
        // or volume start years).
        let n = normalize_dotted_basename(
            "Series.001.(2024).(2023 Reprint).(Digital).cbz",
        );
        assert_eq!(n, "Series 001 (2024).cbz");
    }

    #[test]
    fn normalize_dotted_basename_no_extension_returns_unchanged() {
        // Defensive: a stem-only input has no dot to anchor the ext
        // split. Return unchanged rather than mangle it — the caller
        // (parse_with_normalization) treats no-change as "skip the
        // retry pass."
        assert_eq!(normalize_dotted_basename("README"), "README");
        assert_eq!(normalize_dotted_basename(""), "");
    }

    #[test]
    fn normalize_dotted_basename_collapses_runs_of_whitespace() {
        // Adjacent strips of parens + dots leave multi-space runs;
        // collapse so the patterns' `\s+` matches predictably.
        let n = normalize_dotted_basename("A.B.(2024).(Tag).(Other).cbz");
        assert_eq!(n, "A B (2024).cbz");
    }

    #[test]
    fn parse_with_normalization_claims_user_example() {
        // End-to-end: the user's filename now parses to series +
        // number + year via the cascade. Without the cascade this
        // returned None and Phase B fell through to _unsorted.
        let p = parse_with_normalization(
            "Absolute.Green.Lantern.007.(2025).(Digital).(Shan-Empire).cbz",
            &patterns(),
        )
        .unwrap();
        assert_eq!(p.series_title, "Absolute Green Lantern");
        assert_eq!(p.number, "007");
        assert_eq!(p.year, Some(2025));
    }

    #[test]
    fn parse_with_normalization_prefers_original_on_pattern_hit() {
        // Canonical inputs MUST claim against the original basename,
        // not the normalized one. Skipping this check would silently
        // strip subtitles: id=10 (the permissive pattern) accepts the
        // normalized output `Saga 001 (2014).cbz` and loses the
        // ` - Volume One` capture.
        let p = parse_with_normalization("Saga 001 (2014) - Volume One.cbz", &patterns())
            .unwrap();
        assert_eq!(p.pattern_id, 2, "canonical input must claim against id=2");
        assert_eq!(p.title.as_deref(), Some("Volume One"));
    }

    #[test]
    fn parse_with_normalization_returns_none_when_neither_pass_matches() {
        // Garbage that the original parser rejects AND the normalizer
        // can't rescue (no number anywhere). Caller (Phase B) treats
        // None as "send to _unsorted" — same disposition as before
        // this fix.
        assert!(parse_with_normalization("README.txt", &patterns()).is_none());
        assert!(parse_with_normalization("cover.jpg", &patterns()).is_none());
    }

    #[test]
    fn parse_with_normalization_no_double_work_when_input_lacks_dots() {
        // The `if normalized == basename` short-circuit avoids a
        // redundant pattern pass when normalization is a no-op (input
        // has no dots in the stem). Equivalence at the result level
        // is what matters — same outcome as plain `parse`.
        let raw = "Wolverine (2024) 005.cbz";
        let a = parse(raw, &patterns());
        let b = parse_with_normalization(raw, &patterns());
        assert_eq!(a, b);
    }
}
