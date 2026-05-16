//! `Issue` domain type and `IssueNumber` newtype with natural-comparison ordering.
//!
//! Issue numbers in the comics industry are not integers. They're strings like
//! `1`, `1.MU`, `½`, `Annual 2`, `Year One`, `-1`. [`IssueNumber`] wraps a
//! `String` and exposes [`IssueNumber::natural_cmp`] (an associated function
//! suitable for `sort_by`) and [`IssueNumber::matches`] (natural-comparison
//! equality). [`Ord`] and [`PartialOrd`] are *not* implemented — callers must
//! pass `IssueNumber::natural_cmp` explicitly. This keeps [`Eq`] / [`Hash`] /
//! `==` strictly raw-string-based (so `IssueNumber` is safe as a map key or in
//! a `HashSet`) while letting sort and "is this the same issue?" use the
//! tolerant natural rules:
//!
//! - Bucket A — numeric-leading. Parse leading number (optional sign,
//!   optional `.digits` decimal). The character `½` (and the ASCII
//!   approximation `1/2`) is treated as `0.5`. Order by float value, then by
//!   trailing suffix string.
//! - Bucket B — non-numeric-leading. Parse as `(prefix, optional_trailing_number,
//!   optional_trailing_suffix)`. Order by case-insensitive prefix, then by
//!   trailing number (`None` < `Some(n)`), then by trailing suffix.
//! - Bucket A always sorts before Bucket B.

use std::cmp::Ordering;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// A single issue belonging to a series. Stored in the `issues` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub series_id: i64,
    /// ComicVine issue ID (the trailing integer of the slug, e.g. `12345`
    /// extracted from `4000-12345`).
    pub cv_id: Option<i64>,
    /// Metron issue slug, captured opportunistically from ComicInfo `<Web>`
    /// URLs even though Phase A doesn't call Metron's API.
    pub metron_id: Option<String>,
    pub number: IssueNumber,
    pub title: Option<String>,
    /// ISO-8601 date string `YYYY-MM-DD`, or partial-date string as returned
    /// by ComicVine. Kept as a `String` to preserve whatever upstream sends.
    pub cover_date: Option<String>,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
}

/// Newtype over a raw issue-number string. `Eq` / `Hash` are raw-string based;
/// natural ordering is opt-in via [`IssueNumber::natural_cmp`].
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IssueNumber(String);

impl IssueNumber {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Natural-comparison ordering. Pass directly as a `sort_by` comparator:
    /// `nums.sort_by(IssueNumber::natural_cmp);`
    pub fn natural_cmp(a: &Self, b: &Self) -> Ordering {
        natural_cmp_str(&a.0, &b.0)
    }

    /// Natural-comparison equality. `IssueNumber::new("01").matches(&IssueNumber::new("1"))`
    /// is `true` even though the raw strings differ. Use this when the matcher
    /// is comparing ComicInfo / filename strings against canonical CV strings.
    pub fn matches(&self, other: &Self) -> bool {
        Self::natural_cmp(self, other).is_eq()
    }
}

impl From<&str> for IssueNumber {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for IssueNumber {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl AsRef<str> for IssueNumber {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IssueNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq)]
enum Parsed<'a> {
    Numeric { value: f64, suffix: &'a str },
    Alphabetic {
        prefix: &'a str,
        number: Option<i64>,
        suffix: &'a str,
    },
}

fn parse(raw: &str) -> Parsed<'_> {
    let s = raw.trim();

    // Half-issue shorthand: ½ (U+00BD) and ASCII "1/2".
    if s == "½" || s == "1/2" {
        return Parsed::Numeric {
            value: 0.5,
            suffix: "",
        };
    }

    if let Some((value, end)) = parse_leading_number(s) {
        let suffix = &s[end..];
        // Strip a single leading `.` so "1.MU" yields suffix "MU".
        let suffix = suffix.strip_prefix('.').unwrap_or(suffix);
        return Parsed::Numeric { value, suffix };
    }

    parse_bucket_b(s)
}

fn parse_leading_number(s: &str) -> Option<(f64, usize)> {
    let bytes = s.as_bytes();
    let mut i = 0;

    if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
        i += 1;
    }

    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == int_start {
        return None;
    }

    // Optional `.digits` fractional part. A trailing `.` with no digits is
    // treated as the start of a suffix, not a decimal point.
    if i < bytes.len() && bytes[i] == b'.' {
        let after_dot = i + 1;
        let mut j = after_dot;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > after_dot {
            i = j;
        }
    }

    let value: f64 = s[..i].parse().ok()?;
    Some((value, i))
}

fn parse_bucket_b(s: &str) -> Parsed<'_> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^(.+?)\s+(\d+)([A-Za-z]*)\s*$").expect("static bucket B regex must compile")
    });

    if let Some(caps) = re.captures(s) {
        let prefix = caps.get(1).unwrap().as_str();
        let number = caps.get(2).unwrap().as_str().parse::<i64>().ok();
        let suffix = caps.get(3).unwrap().as_str();
        return Parsed::Alphabetic {
            prefix,
            number,
            suffix,
        };
    }

    Parsed::Alphabetic {
        prefix: s,
        number: None,
        suffix: "",
    }
}

fn natural_cmp_str(a: &str, b: &str) -> Ordering {
    let pa = parse(a);
    let pb = parse(b);

    match (pa, pb) {
        (Parsed::Numeric { .. }, Parsed::Alphabetic { .. }) => Ordering::Less,
        (Parsed::Alphabetic { .. }, Parsed::Numeric { .. }) => Ordering::Greater,
        (
            Parsed::Numeric {
                value: va,
                suffix: sa,
            },
            Parsed::Numeric {
                value: vb,
                suffix: sb,
            },
        ) => va
            .partial_cmp(&vb)
            .unwrap_or(Ordering::Equal)
            .then_with(|| sa.cmp(sb)),
        (
            Parsed::Alphabetic {
                prefix: pa,
                number: na,
                suffix: sa,
            },
            Parsed::Alphabetic {
                prefix: pb,
                number: nb,
                suffix: sb,
            },
        ) => pa
            .to_lowercase()
            .cmp(&pb.to_lowercase())
            .then(na.cmp(&nb))
            .then_with(|| sa.cmp(sb)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> IssueNumber {
        IssueNumber::new(s)
    }

    fn lt(a: &str, b: &str) -> bool {
        IssueNumber::natural_cmp(&n(a), &n(b)).is_lt()
    }

    fn gt(a: &str, b: &str) -> bool {
        IssueNumber::natural_cmp(&n(a), &n(b)).is_gt()
    }

    fn eq(a: &str, b: &str) -> bool {
        IssueNumber::natural_cmp(&n(a), &n(b)).is_eq()
    }

    #[test]
    fn integer_ordering() {
        assert!(lt("1", "2"));
        assert!(lt("2", "10"));
        assert!(lt("9", "10"));
    }

    #[test]
    fn decimal_ordering() {
        assert!(lt("1", "1.5"));
        assert!(lt("1.5", "2"));
        assert!(lt("1.0", "1.5"));
    }

    #[test]
    fn half_issue_sorts_between_zero_and_one() {
        assert!(lt("0", "½"));
        assert!(lt("½", "1"));
        assert!(eq("½", "1/2"));
    }

    #[test]
    fn negative_issue_sorts_below_zero() {
        assert!(lt("-1", "0"));
        assert!(lt("-1", "1"));
    }

    #[test]
    fn decimal_suffix_after_pure_number() {
        // "1" < "1.MU" because same value 1.0, suffix "" < "MU"
        assert!(lt("1", "1.MU"));
        // "1.MU" < "1.5" because "1.5" parses as value 1.5 (decimal consumed).
        assert!(lt("1.MU", "1.5"));
    }

    #[test]
    fn suffix_letter_variants() {
        // "1A" parses to Numeric { 1.0, "A" }
        assert!(lt("1", "1A"));
        assert!(lt("1A", "1B"));
        assert!(lt("1B", "2"));
    }

    #[test]
    fn numeric_sorts_before_alphabetic() {
        assert!(lt("100", "Annual 1"));
        assert!(lt("9999", "Annual 1"));
        assert!(gt("Annual 1", "0"));
    }

    #[test]
    fn alphabetic_with_trailing_number() {
        assert!(lt("Annual 1", "Annual 2"));
        assert!(lt("Annual 2", "Annual 10"));
        assert!(lt("Annual 9", "Annual 10"));
    }

    #[test]
    fn alphabetic_prefix_case_insensitive() {
        assert!(eq("annual 1", "Annual 1"));
        assert!(eq("ANNUAL 2", "annual 2"));
    }

    #[test]
    fn alphabetic_prefix_alphabetical() {
        assert!(lt("Annual 1", "Special 1"));
        assert!(lt("Annual 99", "Special 1"));
    }

    #[test]
    fn alphabetic_with_no_trailing_number() {
        // "Year One" — no trailing digits, parses to Alphabetic { "Year One", None, "" }
        assert!(gt("Year One", "100"));
        assert!(lt("Annual", "Annual 1")); // None < Some(1)
    }

    #[test]
    fn alphabetic_trailing_letter_suffix() {
        // "Annual 1A" → prefix "Annual", number 1, suffix "A"
        assert!(lt("Annual 1", "Annual 1A"));
        assert!(lt("Annual 1A", "Annual 1B"));
        assert!(lt("Annual 1B", "Annual 2"));
    }

    #[test]
    fn sort_full_series_in_natural_order() {
        let mut nums: Vec<IssueNumber> = vec![
            "Annual 1",
            "10",
            "1",
            "½",
            "-1",
            "Annual 10",
            "1.MU",
            "Year One",
            "2",
            "1.5",
        ]
        .into_iter()
        .map(IssueNumber::new)
        .collect();

        nums.sort_by(IssueNumber::natural_cmp);

        let order: Vec<&str> = nums.iter().map(IssueNumber::as_str).collect();
        assert_eq!(
            order,
            vec![
                "-1", "½", "1", "1.MU", "1.5", "2", "10", "Annual 1", "Annual 10", "Year One",
            ]
        );
    }

    #[test]
    fn display_and_as_str_preserve_raw() {
        let i = IssueNumber::new("1.MU");
        assert_eq!(i.as_str(), "1.MU");
        assert_eq!(i.to_string(), "1.MU");
    }

    #[test]
    fn from_str_and_string_constructors() {
        let a: IssueNumber = "5".into();
        let b: IssueNumber = String::from("5").into();
        assert_eq!(a, b);
    }

    #[test]
    fn whitespace_around_input_ignored_for_ordering() {
        assert!(eq(" 1 ", "1"));
        assert!(eq("  Annual  2  ", "Annual 2"));
    }

    #[test]
    fn matches_treats_leading_zeros_as_equal() {
        assert!(IssueNumber::new("01").matches(&IssueNumber::new("1")));
        assert!(IssueNumber::new("001").matches(&IssueNumber::new("1")));
        assert!(IssueNumber::new("1.0").matches(&IssueNumber::new("1")));
    }

    #[test]
    fn matches_distinguishes_suffixed_variants() {
        // 1 and 1.MU share a numeric value but differ on suffix.
        assert!(!IssueNumber::new("1").matches(&IssueNumber::new("1.MU")));
        assert!(!IssueNumber::new("1").matches(&IssueNumber::new("1A")));
    }

    #[test]
    fn raw_string_eq_independent_of_natural_eq() {
        // Eq is strict raw-string; natural_cmp is tolerant.
        let a = IssueNumber::new("01");
        let b = IssueNumber::new("1");
        assert_ne!(a, b, "PartialEq must use raw string");
        assert!(a.matches(&b), "matches must use natural ordering");
    }
}
