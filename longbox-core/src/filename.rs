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
pub fn default_patterns() -> Vec<ParsingPattern> {
    // Priority order matches the longbox-db migration seed: the volume-aware
    // pattern runs FIRST (priority 5) so it can claim filenames with volume
    // markers before the looser patterns absorb them.
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
}
