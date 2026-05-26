//! Pure filename pattern matching. Takes the basename of a comic file and a
//! slice of [`ParsingPattern`]s (typically loaded from the `parsing_patterns`
//! table) and returns the first successful capture, ordered by `priority`
//! ascending. Patterns with `enabled = false` are skipped. Patterns that fail
//! to compile are skipped silently — the caller has more context for logging.

use regex::Regex;
use serde::{Deserialize, Serialize};

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
}
