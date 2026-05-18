//! [`LibraryPath`] — pure encoding of Phase B's library naming convention.
//!
//! The convention is hard-coded in `longbox-phase-b-prompt.md` and is the
//! single source of truth for where a matched file lives in the library:
//!
//! ```text
//! {library_root}/{series} ({year})/{series} ({year}) {issue:03}.cbz
//! ```
//!
//! When `start_year` is None, both the folder and filename drop the
//! parenthesized year segment entirely:
//!
//! ```text
//! {library_root}/{series}/{series} {issue:03}.cbz
//! ```
//!
//! This module owns the convention itself. It does *not* enforce the
//! year-source policy (Phase B's "use `series.start_year`, never
//! `issue.cover_date.year()`" rule) — that's a caller-boundary concern
//! and would couple this struct to the catalog schema.

use std::path::{Path, PathBuf};

/// Sentinel returned when sanitization strips a series name down to
/// nothing usable. Picked deliberately so the file still lands in a
/// coherent location rather than at the library root.
const UNKNOWN_SERIES: &str = "Unknown";

/// A file's target location in the library. Constructed from
/// catalog-derived inputs; sanitization happens once at construction so
/// `folder()` and `filename()` are pure reads.
///
/// All public methods are pure — no I/O, no allocations beyond the
/// returned strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryPath {
    /// Filesystem-safe series name. Built from the raw input by
    /// [`sanitize_series`]; never empty (falls back to `"Unknown"`).
    series: String,
    /// Year the volume launched. None drops the parens from both folder
    /// and filename per the convention.
    start_year: Option<i32>,
    /// Issue number in display form. Pure integers (including 0 and
    /// negatives) are zero-padded to a 3-digit width; non-integers pass
    /// through verbatim per [`format_issue_number`].
    number: String,
}

impl LibraryPath {
    /// Build from raw catalog fields. The series name is sanitized
    /// (filesystem-unsafe chars stripped, whitespace collapsed, trailing
    /// dots+spaces removed, `Unknown` fallback) and the issue number is
    /// formatted (3-digit zero-pad for integers, verbatim otherwise).
    pub fn new(
        series: impl Into<String>,
        start_year: Option<i32>,
        number: impl Into<String>,
    ) -> Self {
        Self {
            series: sanitize_series(&series.into()),
            start_year,
            number: format_issue_number(&number.into()),
        }
    }

    /// Folder name relative to the library root.
    pub fn folder(&self) -> String {
        match self.start_year {
            Some(year) => format!("{} ({})", self.series, year),
            None => self.series.clone(),
        }
    }

    /// Filename including extension. Folder is *not* included.
    pub fn filename(&self) -> String {
        match self.start_year {
            Some(year) => format!("{} ({}) {}.cbz", self.series, year, self.number),
            None => format!("{} {}.cbz", self.series, self.number),
        }
    }

    /// `library_root / folder() / filename()`.
    pub fn full(&self, library_root: &Path) -> PathBuf {
        library_root.join(self.folder()).join(self.filename())
    }
}

/// Filesystem-unsafe characters per the Phase B convention, plus the
/// defensive extensions agreed during Step 2 kickoff: control chars and
/// trailing-dot stripping (Windows / SMB safety even on the macOS-only
/// container today).
fn sanitize_series(raw: &str) -> String {
    // 1. Replace unsafe chars + drop controls. Each unsafe char becomes
    //    a single space; consecutive whitespace is collapsed in step 2.
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        let replaced = match c {
            ':' | '/' | '\\' | '?' | '*' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => continue, // drop control / DEL chars entirely
            c => c,
        };
        out.push(replaced);
    }

    // 2. Collapse Unicode whitespace runs to a single ASCII space.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_ws = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                collapsed.push(' ');
            }
            prev_ws = true;
        } else {
            collapsed.push(c);
            prev_ws = false;
        }
    }

    // 3. Trim leading whitespace and trailing whitespace+dots.
    //    Trailing dots iteratively until neither remains (handles
    //    "Name. . ." → "Name").
    let trimmed = collapsed.trim_start();
    let mut end = trimmed.trim_end();
    loop {
        let after = end.trim_end_matches('.').trim_end();
        if after.len() == end.len() {
            break;
        }
        end = after;
    }

    // 4. Edge cases: empty, or pathological `.` / `..` as the whole name.
    if end.is_empty() || end == "." || end == ".." {
        return UNKNOWN_SERIES.to_string();
    }
    end.to_string()
}

/// Zero-pad pure-integer issue numbers to 3 digits (preserving sign as
/// a separate prefix), pass non-integers through verbatim.
///
/// Examples:
/// - `"1"` → `"001"`, `"0"` → `"000"`, `"01"` → `"001"`
/// - `"-1"` → `"-001"` (sign stays separate from the digit width)
/// - `"1000"` → `"1000"` (no truncation; pad is a minimum width)
/// - `"½"`, `"Annual 1"`, `"3A"`, `"1.MU"` → returned verbatim
fn format_issue_number(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(n) = trimmed.parse::<i64>() {
        return if n < 0 {
            format!("-{:03}", n.unsigned_abs())
        } else {
            format!("{:03}", n)
        };
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ----- happy path / full composition -----

    #[test]
    fn happy_path_with_year() {
        let p = LibraryPath::new("Wolverine: Origin", Some(2001), "1");
        assert_eq!(p.folder(), "Wolverine Origin (2001)");
        assert_eq!(p.filename(), "Wolverine Origin (2001) 001.cbz");
    }

    #[test]
    fn full_joins_library_root() {
        let p = LibraryPath::new("Saga", Some(2012), "1");
        let full = p.full(Path::new("/library"));
        assert_eq!(full, Path::new("/library/Saga (2012)/Saga (2012) 001.cbz"));
    }

    #[test]
    fn missing_year_drops_parens_from_both() {
        let p = LibraryPath::new("Saga", None, "1");
        assert_eq!(p.folder(), "Saga");
        assert_eq!(p.filename(), "Saga 001.cbz");
        assert_eq!(
            p.full(Path::new("/lib")),
            Path::new("/lib/Saga/Saga 001.cbz")
        );
    }

    // ----- filesystem-unsafe character handling -----

    #[test]
    fn each_unsafe_char_becomes_space() {
        for c in [':', '/', '\\', '?', '*', '"', '<', '>', '|'] {
            let raw = format!("Name{c}With{c}Char");
            let p = LibraryPath::new(&raw, None, "1");
            assert_eq!(
                p.folder(),
                "Name With Char",
                "char {c:?} not collapsed correctly"
            );
        }
    }

    #[test]
    fn consecutive_unsafe_chars_collapse_to_single_space() {
        let p = LibraryPath::new("Title??: <Part>", None, "1");
        assert_eq!(p.folder(), "Title Part");
    }

    #[test]
    fn unicode_nbsp_and_tabs_collapse_with_ascii_space() {
        // NBSP (U+00A0), tab, regular space all collapse into one ASCII.
        let p = LibraryPath::new("A\u{00A0}\t  B", None, "1");
        assert_eq!(p.folder(), "A B");
    }

    #[test]
    fn control_chars_stripped() {
        // Embedded NUL, bell, DEL all dropped silently.
        let p = LibraryPath::new("Saga\0\u{0007}\u{007F}", Some(2012), "1");
        assert_eq!(p.folder(), "Saga (2012)");
    }

    #[test]
    fn trailing_dots_and_spaces_stripped() {
        // Windows / SMB silently strips these; mirror that explicitly.
        let p = LibraryPath::new("Series Name. . .", Some(2024), "1");
        assert_eq!(p.folder(), "Series Name (2024)");
    }

    #[test]
    fn empty_after_sanitization_falls_back_to_unknown() {
        // All chars stripped → empty → "Unknown".
        let p = LibraryPath::new("?????", Some(2024), "1");
        assert_eq!(p.folder(), "Unknown (2024)");
    }

    #[test]
    fn pathological_dots_fall_back_to_unknown() {
        for raw in [".", ".."] {
            let p = LibraryPath::new(raw, None, "1");
            assert_eq!(p.folder(), "Unknown", "raw={raw:?}");
        }
    }

    #[test]
    fn whitespace_only_input_falls_back_to_unknown() {
        let p = LibraryPath::new("   \t  ", Some(2024), "1");
        assert_eq!(p.folder(), "Unknown (2024)");
    }

    // ----- integer issue number padding -----

    #[test]
    fn pure_integer_padded_to_three_digits() {
        for (input, expected) in [
            ("0", "Saga (2012) 000.cbz"),   // 0 = preview issues in '90s comics
            ("1", "Saga (2012) 001.cbz"),
            ("12", "Saga (2012) 012.cbz"),
            ("100", "Saga (2012) 100.cbz"),
            ("1000", "Saga (2012) 1000.cbz"), // wider than 3 → natural width
        ] {
            let p = LibraryPath::new("Saga", Some(2012), input);
            assert_eq!(p.filename(), expected, "input={input:?}");
        }
    }

    #[test]
    fn negative_integer_pads_digits_keeps_sign() {
        // Marvel #-1 events / similar. Sign isn't a digit; 3-pad applies
        // to digits only.
        for (input, expected_num) in [("-1", "-001"), ("-12", "-012"), ("-100", "-100")] {
            let p = LibraryPath::new("X-Men", Some(1991), input);
            let fname = p.filename();
            assert!(
                fname.contains(expected_num),
                "input={input:?} expected {expected_num:?} in {fname}"
            );
        }
    }

    #[test]
    fn leading_zero_input_normalized() {
        let p = LibraryPath::new("Saga", Some(2012), "01");
        assert_eq!(p.filename(), "Saga (2012) 001.cbz");
    }

    #[test]
    fn whitespace_around_number_trimmed() {
        let p = LibraryPath::new("Saga", Some(2012), "  5  ");
        assert_eq!(p.filename(), "Saga (2012) 005.cbz");
    }

    // ----- non-integer issue number pass-through -----

    #[test]
    fn non_integer_numbers_pass_through_verbatim() {
        for raw in ["½", "Annual 1", "1.5", "1.MU", "3A", "Year One"] {
            let p = LibraryPath::new("Saga", Some(2012), raw);
            let expected = format!("Saga (2012) {raw}.cbz");
            assert_eq!(p.filename(), expected, "raw={raw:?}");
        }
    }
}
