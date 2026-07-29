//! Reading a comic's identity out of the path names *inside* its archive.
//!
//! Scene releases bake a label into their internal paths — either a single
//! top-level folder (`My Little Warlord 008/`) or the page filenames
//! themselves (`Void Rivals 001-000.jpg`). That label is written once at
//! release time and then never touched again, which makes it the only
//! identity evidence that is independent of everything else LongBox knows:
//!
//! - independent of the **filename**, which users and tools rename freely,
//!   and which is where the wrong bindings came from in the first place
//!   (most bindings were derived from it, so filename agreement is circular);
//! - independent of the **catalog binding**, which is the thing under audit;
//! - independent of **ComicInfo**, which is separately known to lie.
//!
//! It is not universal — it only exists because a release group put it there,
//! and plenty of archives have generic `page-001.jpg` naming that says
//! nothing. So every function here degrades to "no signal" rather than
//! guessing. A wrong answer feeds a keeper decision that deletes files.

use crate::normalize::normalize_title;

/// Where the label came from, which changes how its trailing number reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    /// A single top-level directory inside the archive. Its trailing number
    /// is the issue.
    Dir,
    /// A page filename. Its trailing number is the PAGE, not the issue —
    /// reading it as an issue number is the mistake that makes this whole
    /// technique look like it works while being wrong.
    Page,
}

/// What an internal label claims the archive is.
///
/// Both halves are RAW EVIDENCE, not conclusions. In particular `series` must
/// be resolved against the catalog before it means anything: a generically
/// named archive yields junk like `page-001`, and treating unresolvable text
/// as "filed under the wrong series" would flag every such archive in the
/// library. Only a series text that names a series the catalog actually has
/// is evidence about where the content belongs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArchiveIdentity {
    /// Series text, normalized. `None` when nothing usable remained.
    pub series: Option<String>,
    /// Issue number with leading zeros stripped. `None` when no number
    /// survived the conservative filters.
    pub issue: Option<String>,
}

const IMAGE_EXTS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "gif"];

fn is_image(name: &str) -> bool {
    // macOS resource forks are not pages. They shadow every real image under
    // a `__MACOSX/` tree, which breaks the single-top-directory rule and
    // demotes a good `Dir` label to a weaker `Page` one for free.
    if name.starts_with("__MACOSX/") || name.contains("/__MACOSX/") {
        return false;
    }
    let base = name.rsplit('/').next().unwrap_or(name);
    if base.starts_with("._") {
        return false;
    }
    let lower = base.to_ascii_lowercase();
    IMAGE_EXTS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Pick the label out of an archive's entry names.
///
/// Prefers a single common top-level directory: it names the whole archive,
/// so its trailing number is the issue. Falls back to the first page's
/// filename, whose trailing number is a page counter.
///
/// `None` when there are no image entries, or when images sit at several
/// different top levels — a shape we have no confident reading of.
pub fn label_from_entries(names: &[String]) -> Option<(String, LabelKind)> {
    let images: Vec<&String> = names.iter().filter(|n| is_image(n)).collect();
    if images.is_empty() {
        return None;
    }

    let tops: std::collections::BTreeSet<&str> = images
        .iter()
        .filter_map(|n| n.split_once('/').map(|(top, _)| top))
        .collect();

    // Every image under exactly one directory → that directory is the label.
    if tops.len() == 1 && images.iter().all(|n| n.contains('/')) {
        let top = tops.into_iter().next()?;
        if !top.trim().is_empty() {
            return Some((top.to_owned(), LabelKind::Dir));
        }
    }

    // Otherwise use the lexicographically first page's stem — stable
    // regardless of the order the archive happens to store entries in.
    let mut basenames: Vec<&str> = images
        .iter()
        .map(|n| n.rsplit('/').next().unwrap_or(n.as_str()))
        .collect();
    basenames.sort_unstable();
    let stem = basenames
        .first()?
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(basenames[0]);
    (!stem.trim().is_empty()).then(|| (stem.to_owned(), LabelKind::Page))
}

/// Parse a label into the series and issue it claims.
///
/// Conservative by construction — every rule here drops evidence rather than
/// inventing it:
///
/// - Parenthesised and bracketed groups are removed wholesale. They carry
///   years, scan credits and format tags (`(2025) (Digital) (Mephisto-Empire)`),
///   none of which is identity.
/// - Four-digit numbers in `19xx`/`20xx` are treated as years, never issues.
/// - For a [`LabelKind::Page`] label the trailing number is discarded as a
///   page counter before anything else is considered.
/// - Whatever is left before the issue number is the series text.
///
/// Either half can come back `None` independently: `Blood Train (one-shot)
/// (2025) (Digital) (Mephisto-Empire)` yields a series and no issue, which is
/// still decisive evidence when the file is filed under a different series.
pub fn parse_label(label: &str, kind: LabelKind) -> ArchiveIdentity {
    let cleaned = strip_bracketed(label);

    let mut numbers: Vec<(usize, &str)> = Vec::new();
    for (idx, tok) in number_tokens(&cleaned) {
        if !looks_like_year(tok) {
            numbers.push((idx, tok));
        }
    }

    // A page filename's last number counts pages, not issues. Remember where
    // it started: with no issue number left, the series text has to stop
    // there, or the page counter ends up glued onto the series name
    // (`Blood Train - 001` → series `blood train - 001`).
    // A page label with three or more numbers is a shape we cannot read
    // confidently. Double-page spreads are named `<issue>-<page>-<page>`, so
    // popping exactly one trailing number leaves a PAGE number standing where
    // the issue should be — `008-014-015` would report issue 14. Refusing to
    // guess is the same rule as everywhere else here.
    if kind == LabelKind::Page && numbers.len() >= 3 {
        let series = {
            let head = cleaned[..numbers[0].0].trim().trim_end_matches(['-', '_', '.']);
            let n = normalize_title(head);
            (!n.is_empty()).then_some(n)
        };
        return ArchiveIdentity {
            series,
            issue: None,
        };
    }

    let mut page_number_at = None;
    if kind == LabelKind::Page {
        if let Some((idx, _)) = numbers.pop() {
            page_number_at = Some(idx);
        }
    }

    let (issue, series_end) = match numbers.last() {
        Some((idx, tok)) => (Some(strip_leading_zeros(tok)), *idx),
        None => (None, page_number_at.unwrap_or(cleaned.len())),
    };

    let series_text = cleaned[..series_end].trim().trim_end_matches(['-', '_', '.']);
    let series = {
        let normalized = normalize_title(series_text.trim());
        (!normalized.is_empty()).then_some(normalized)
    };

    ArchiveIdentity { series, issue }
}

/// Remove `(...)` and `[...]` spans, replacing each with a space so tokens on
/// either side stay separate. Unbalanced openers swallow the rest of the
/// string, which is the conservative reading of a truncated label.
fn strip_bracketed(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                out.push(' ');
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Runs of 1–4 digits with their byte offsets, optionally followed by a
/// decimal part (`029.5`). Longer integer runs are not issue numbers in any
/// real-world naming scheme and are ignored entirely.
///
/// The decimal part is not optional polish: point issues are real catalog
/// rows, and splitting `029.5` into `029` and `5` makes the parser confidently
/// report issue 5 for a Radiant Black 29.5 archive — a wrong answer that
/// feeds a keeper decision, which is worse than no answer.
fn number_tokens(s: &str) -> Vec<(usize, &str)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let int_len = i - start;
            // Absorb a `.N` / `.NN` suffix when digits actually follow the dot.
            if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() && j - i <= 2 {
                    j += 1;
                }
                i = j;
            }
            if (1..=4).contains(&int_len) {
                out.push((start, &s[start..i]));
            }
        } else {
            i += 1;
        }
    }
    out
}

fn looks_like_year(tok: &str) -> bool {
    tok.len() == 4
        && !tok.contains('.')
        && (tok.starts_with("19") || tok.starts_with("20"))
}

/// `029` → `29`, `029.5` → `29.5`, `000` → `0`. Only the integer part is
/// stripped, so a point issue keeps its fraction.
fn strip_leading_zeros(tok: &str) -> String {
    let (int_part, frac) = match tok.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (tok, None),
    };
    let t = int_part.trim_start_matches('0');
    let head = if t.is_empty() { "0" } else { t };
    match frac {
        Some(f) => format!("{head}.{f}"),
        None => head.to_owned(),
    }
}

/// Does a label's series text name the same series as a catalog title?
///
/// Not plain equality on normalized titles. Release groups punctuate subtitles
/// with a hyphen where the catalog uses a space — `Marvel Knights - Punisher`
/// vs `Marvel Knights Punisher`, `Wynd - The Power of the Blood` vs `Wynd The
/// Power of the Blood` — and `normalize_title` deliberately preserves hyphens
/// (they are load-bearing in `X-Men`, `Spider-Man`). Comparing normalized
/// titles directly reported six of this library's groups as filed under the
/// wrong series when nothing was wrong at all.
///
/// So for this comparison only, hyphens are separators. A false "same series"
/// costs nothing here — the issue number still has to agree — while a false
/// "different series" manufactures a misfiling report out of punctuation.
pub fn series_matches(label_series: &str, catalog_title: &str) -> bool {
    fn key(s: &str) -> String {
        normalize_title(s)
            .replace('-', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    let (a, b) = (key(label_series), key(catalog_title));
    !a.is_empty() && a == b
}

/// Convenience: entries → identity, in one step. `None` when no label could
/// be read at all.
pub fn identity_from_entries(names: &[String]) -> Option<ArchiveIdentity> {
    let (label, kind) = label_from_entries(names)?;
    let id = parse_label(&label, kind);
    (id.series.is_some() || id.issue.is_some()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn single_top_directory_is_the_label_and_its_number_is_the_issue() {
        let names = v(&[
            "My Little Warlord 008/008-0000.jpg",
            "My Little Warlord 008/008-0001.jpg",
        ]);
        let (label, kind) = label_from_entries(&names).unwrap();
        assert_eq!(label, "My Little Warlord 008");
        assert_eq!(kind, LabelKind::Dir);
        let id = parse_label(&label, kind);
        assert_eq!(id.issue.as_deref(), Some("8"));
        assert_eq!(id.series.as_deref(), Some("my little warlord"));
    }

    /// The mistake that makes this technique look like it works while being
    /// wrong: a page filename's trailing number counts pages.
    #[test]
    fn a_page_labels_trailing_number_is_the_page_not_the_issue() {
        let id = parse_label("My Little Warlord 008-0000", LabelKind::Page);
        assert_eq!(id.issue.as_deref(), Some("8"), "008 is the issue");
        let id = parse_label("Void Rivals 001-000", LabelKind::Page);
        assert_eq!(id.issue.as_deref(), Some("1"));
        // Read as a Dir label the same string would give the page number.
        assert_eq!(
            parse_label("Void Rivals 001-000", LabelKind::Dir).issue.as_deref(),
            Some("0"),
            "this is exactly the misreading LabelKind exists to prevent"
        );
    }

    #[test]
    fn two_digit_issue_with_page_suffix() {
        // Real: Carmen Red Claw.
        let id = parse_label("Carmen Red Claw - Belly of the Beast 02-0000", LabelKind::Page);
        assert_eq!(id.issue.as_deref(), Some("2"));
        assert_eq!(
            id.series.as_deref(),
            Some("carmen red claw - belly of the beast")
        );
    }

    #[test]
    fn space_dash_space_before_the_page_counter() {
        // Real: Wynd. The year in parens must not be read as the issue.
        let id = parse_label(
            "Wynd - The Power of the Blood 004 (2025) - 0001",
            LabelKind::Page,
        );
        assert_eq!(id.issue.as_deref(), Some("4"));
        assert_eq!(id.series.as_deref(), Some("wynd - the power of the blood"));
    }

    #[test]
    fn scan_credits_and_years_are_not_issue_numbers() {
        let id = parse_label(
            "Something is Killing the Children 045 (2026) (digital) (Goobadaddy-Empire)",
            LabelKind::Dir,
        );
        assert_eq!(id.issue.as_deref(), Some("45"));
        assert_eq!(
            id.series.as_deref(),
            Some("something is killing the children")
        );
    }

    /// Jeremy's Blood Train case: no issue number survives, but the SERIES
    /// does — and that alone proves the file is filed under the wrong series.
    #[test]
    fn a_label_with_no_number_still_yields_a_series() {
        let id = parse_label(
            "Blood Train (one-shot) (2025) (Digital) (Mephisto-Empire)",
            LabelKind::Dir,
        );
        assert_eq!(id.issue, None, "no issue number to find");
        assert_eq!(id.series.as_deref(), Some("blood train"));
    }

    /// Generic naming yields no issue, and series text that is junk.
    ///
    /// This is why the series half is RAW EVIDENCE, not a conclusion: the
    /// caller must resolve it against the catalog and ignore anything that
    /// names no real series. Treating unresolvable text as "filed under the
    /// wrong series" would flag every generically-named archive in the
    /// library.
    #[test]
    fn generic_page_naming_yields_no_issue_and_unresolvable_series_text() {
        let names = v(&["page-001.jpg", "page-002.jpg"]);
        let (label, kind) = label_from_entries(&names).unwrap();
        assert_eq!(kind, LabelKind::Page);
        let id = parse_label(&label, kind);
        assert_eq!(id.issue, None, "the only number was the page counter");
        // Junk, and it must stay junk rather than being laundered into a
        // claim — no catalog series normalizes to this.
        assert_eq!(id.series.as_deref(), Some("page"));
    }

    /// A page-named archive can still carry a real series, so the parser must
    /// not blanket-discard series text just because the kind is Page.
    #[test]
    fn a_page_label_can_still_carry_a_real_series() {
        let id = parse_label("Blood Train - 001", LabelKind::Page);
        assert_eq!(id.issue, None, "001 is the page counter here");
        assert_eq!(id.series.as_deref(), Some("blood train"));
    }

    #[test]
    fn no_images_means_no_label() {
        assert!(label_from_entries(&v(&["ComicInfo.xml", "notes.txt"])).is_none());
        assert!(label_from_entries(&[]).is_none());
    }

    #[test]
    fn images_at_several_top_levels_fall_back_to_a_page_name() {
        let names = v(&["a/001.jpg", "b/001.jpg"]);
        let (_, kind) = label_from_entries(&names).unwrap();
        assert_eq!(kind, LabelKind::Page);
    }

    #[test]
    fn mixed_root_and_nested_images_do_not_claim_a_directory_label() {
        // One image at the root means the directory does not describe the
        // whole archive, so it must not be read as the label.
        let names = v(&["Series 001/001.jpg", "cover.jpg"]);
        let (_, kind) = label_from_entries(&names).unwrap();
        assert_eq!(kind, LabelKind::Page);
    }

    /// Point issues are real catalog rows. Splitting `029.5` into `029` and
    /// `5` made the parser confidently report issue 5 for a Radiant Black
    /// 29.5 archive — a confident wrong answer feeding a keeper decision.
    #[test]
    fn decimal_issue_numbers_survive_as_one_number() {
        let id = parse_label(
            "Radiant Black 029.5 (2024) (Digital) (Shan-Empire)",
            LabelKind::Dir,
        );
        assert_eq!(id.issue.as_deref(), Some("29.5"));
        assert_eq!(id.series.as_deref(), Some("radiant black"));
    }

    #[test]
    fn a_lone_dot_does_not_start_a_decimal() {
        // Trailing dot with no digits after it is punctuation, not a fraction.
        let id = parse_label("Series 007. Something", LabelKind::Dir);
        assert_eq!(id.issue.as_deref(), Some("7"));
    }

    /// Six real groups reported as "wrong series" purely because the release
    /// group hyphenated a subtitle the catalog spaces.
    #[test]
    fn series_matching_treats_hyphens_as_separators() {
        assert!(series_matches(
            "marvel knights - punisher",
            "Marvel Knights Punisher"
        ));
        assert!(series_matches(
            "wynd - the power of the blood",
            "Wynd The Power of the Blood"
        ));
        assert!(series_matches(
            "carmen red claw - belly of the beast",
            "Carmen Red Claw Belly of the Beast"
        ));
        // Genuinely different series still do not match.
        assert!(!series_matches("blood train", "Book of Cutter"));
        // Junk from generic page naming resolves to nothing.
        assert!(!series_matches("page", "Hello Darkness"));
        assert!(!series_matches("", "Anything"));
    }

    /// Double-page spreads are named `<issue>-<page>-<page>`, so popping one
    /// trailing number would leave a PAGE number standing where the issue
    /// belongs and report issue 14 for an issue-8 archive.
    #[test]
    fn a_spread_page_name_yields_no_issue_rather_than_a_wrong_one() {
        let id = parse_label("008-014-015", LabelKind::Page);
        assert_eq!(id.issue, None, "must refuse rather than report page 14");
        // Two numbers is the ordinary `<issue>-<page>` shape and still works.
        assert_eq!(
            parse_label("008-0000", LabelKind::Page).issue.as_deref(),
            Some("8")
        );
        // The series half survives a refusal.
        let id = parse_label("My Little Warlord 008-014-015", LabelKind::Page);
        assert_eq!(id.issue, None);
        assert_eq!(id.series.as_deref(), Some("my little warlord"));
    }

    #[test]
    fn macos_resource_forks_are_not_pages() {
        // Without filtering, the `__MACOSX` shadow tree adds a second top
        // level and demotes a perfectly good Dir label to Page.
        let names = v(&[
            "My Little Warlord 008/008-0000.jpg",
            "My Little Warlord 008/008-0001.jpg",
            "__MACOSX/My Little Warlord 008/._008-0000.jpg",
        ]);
        let (label, kind) = label_from_entries(&names).unwrap();
        assert_eq!(kind, LabelKind::Dir, "resource forks must not split the tree");
        assert_eq!(label, "My Little Warlord 008");
    }

    #[test]
    fn long_digit_runs_are_ignored() {
        // A 6-digit run is not an issue number in any real naming scheme.
        let id = parse_label("Series 123456", LabelKind::Dir);
        assert_eq!(id.issue, None);
    }

    #[test]
    fn identity_from_entries_requires_at_least_one_half() {
        assert!(identity_from_entries(&v(&["ComicInfo.xml"])).is_none());
        let id = identity_from_entries(&v(&["Saga 001/000.jpg"])).unwrap();
        assert_eq!(id.issue.as_deref(), Some("1"));
    }
}
