//! Title normalization. Applied at series-insert time to populate
//! `series.sort_title`, and at match time to the candidate ComicInfo `<Series>`
//! text or filename-parsed series text. Both sides of the comparison live in
//! the same normalized space.

use unicode_normalization::UnicodeNormalization;

/// NFC-normalize, lowercase, strip a single leading "the"/"a"/"an" article,
/// strip a trailing `" by <author>"` author-attribution annotation, drop
/// punctuation except hyphens, collapse runs of whitespace.
///
/// The NFC step folds decomposed combining marks (e.g. `e\u{301}`) into
/// precomposed code points (`é`) so a filename pulled from macOS's HFS+
/// NFD storage compares equal to a CV-sourced sort_title in NFC. Without
/// this, `Stillwater by Zdarsky & Pérez 012.cbr` (NFD on disk) scored
/// 0.0 against the catalog's `Stillwater` row.
///
/// The `" by ..."` strip handles Amazon-scraped ComicInfo and verbose
/// scene names that suffix the series title with an author attribution
/// (`Stillwater by Zdarsky & Pérez`). The catalog row is just
/// `Stillwater`; without the strip the token-set Jaccard scores
/// 1/4 = 0.25 and the file dead-ends as unmatched. Applied at both
/// insert and match time so the invariant holds end-to-end.
pub fn normalize_title(input: &str) -> String {
    let nfc: String = input.nfc().collect();
    let lower = nfc.to_lowercase();
    let stripped = strip_leading_article(&lower);
    let without_attribution = strip_author_attribution(stripped);
    let cleaned = drop_punctuation(without_attribution);
    collapse_whitespace(&cleaned)
}

/// Did [`normalize_title`] drop a trailing `" by <authors>"` from this title?
///
/// Exposed so callers can distinguish "same series, differently spelled" from
/// "a collected edition named after its creators" — `Fire Power By Kirkman
/// And Samnee (2023)` normalizes to the same thing as `Fire Power (2020)`
/// precisely BECAUSE this strip fires, so the strip firing is the signal.
///
/// Reuses the same predicate normalization uses rather than re-deriving it: a
/// second definition of "is this an author attribution" would disagree with
/// production somewhere, and every disagreement would become a misfiled
/// finding.
pub fn has_author_attribution(title: &str) -> bool {
    let nfc: String = title.nfc().collect();
    let lower = nfc.to_lowercase();
    let stripped = strip_leading_article(&lower);
    strip_author_attribution(stripped).len() != stripped.len()
}

fn strip_leading_article(s: &str) -> &str {
    for prefix in ["the ", "a ", "an "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return rest;
        }
    }
    s
}

/// Trim a trailing `" by ..."` author-attribution suffix. Be conservative
/// — many series carry "by" as part of the title:
///   - `Step By Bloody Step`
///   - `The Nice House by the Sea`
///   - `Touched by a Demon`
///   - `Door to Door, Night by Night`
///
/// Only strip when the suffix has a hallmark of a multi-author list:
/// `&`, a comma, or a free-standing ` and `. Misses single-author shapes
/// like `Series by Brian Wood` — an acceptable trade-off; single-author
/// verbose ComicInfo titles are rare, and corrupting legitimate
/// `by`-in-title series would be worse.
fn strip_author_attribution(s: &str) -> &str {
    let Some(idx) = s.rfind(" by ") else {
        return s;
    };
    let head = &s[..idx];
    if head.trim().is_empty() {
        return s;
    }
    let suffix = &s[idx + 4..];
    let looks_like_attribution =
        suffix.contains('&') || suffix.contains(',') || suffix.contains(" and ");
    if looks_like_attribution {
        head
    } else {
        s
    }
}

fn drop_punctuation(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '-' || c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect()
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_attribution_is_detectable_by_the_same_rule_that_strips_it() {
        assert!(has_author_attribution("Fire Power By Kirkman And Samnee"));
        assert!(has_author_attribution("Stillwater by Zdarsky & Pérez"));
        // The conservative cases normalization deliberately leaves alone.
        assert!(!has_author_attribution("Step By Bloody Step"));
        assert!(!has_author_attribution("The Nice House by the Sea"));
        assert!(!has_author_attribution("Series by Brian Wood"));
        assert!(!has_author_attribution("Fire Power"));
    }

    #[test]
    fn lowercase_only() {
        assert_eq!(normalize_title("Saga"), "saga");
        assert_eq!(normalize_title("SAGA"), "saga");
    }

    #[test]
    fn strips_leading_the() {
        assert_eq!(normalize_title("The Walking Dead"), "walking dead");
        assert_eq!(normalize_title("the walking dead"), "walking dead");
    }

    #[test]
    fn strips_leading_a_and_an() {
        assert_eq!(normalize_title("A Team"), "team");
        assert_eq!(normalize_title("An Unkindness"), "unkindness");
    }

    #[test]
    fn does_not_strip_article_without_trailing_space() {
        assert_eq!(
            normalize_title("Theory of Everything"),
            "theory of everything"
        );
        assert_eq!(normalize_title("Ant-Man"), "ant-man");
        assert_eq!(normalize_title("The"), "the");
    }

    #[test]
    fn preserves_internal_hyphens() {
        assert_eq!(normalize_title("X-Men"), "x-men");
        assert_eq!(normalize_title("Spider-Man"), "spider-man");
        assert_eq!(normalize_title("Cable & X-Force"), "cable x-force");
    }

    #[test]
    fn drops_colons_and_apostrophes_and_periods() {
        assert_eq!(
            normalize_title("Batman: The Animated Series"),
            "batman the animated series"
        );
        assert_eq!(normalize_title("Devil's Reign"), "devil s reign");
        assert_eq!(normalize_title("X.Y.Z"), "x y z");
    }

    #[test]
    fn drops_hash_and_other_symbols() {
        assert_eq!(normalize_title("Action Comics #1000"), "action comics 1000");
        assert_eq!(normalize_title("Saga / Volume 1"), "saga volume 1");
    }

    #[test]
    fn collapses_multiple_spaces() {
        assert_eq!(normalize_title("  Saga   "), "saga");
        assert_eq!(normalize_title("Saga    Vol    1"), "saga vol 1");
        assert_eq!(normalize_title("Saga\tVol\n1"), "saga vol 1");
    }

    #[test]
    fn handles_empty_and_whitespace_only() {
        assert_eq!(normalize_title(""), "");
        assert_eq!(normalize_title("   "), "");
        assert_eq!(normalize_title("the "), "");
    }

    #[test]
    fn handles_unicode_letters() {
        assert_eq!(normalize_title("Pokémon"), "pokémon");
        assert_eq!(normalize_title("Café"), "café");
    }

    #[test]
    fn nfc_folds_decomposed_accents_to_precomposed() {
        // Decomposed form: `é` as `e` + combining-acute (\u{0301}).
        // Precomposed form: literal `é` (\u{00e9}).
        // Both must produce the same normalized string so a filename
        // pulled from macOS HFS+ (NFD) compares equal to a CV-sourced
        // sort_title (NFC). Before this fix the live `Stillwater by
        // Zdarsky & Pérez` files dead-ended as unmatched.
        let decomposed = "Pe\u{0301}rez";
        let precomposed = "Pérez";
        assert_eq!(normalize_title(decomposed), normalize_title(precomposed));
    }

    #[test]
    fn strips_trailing_by_attribution_with_ampersand() {
        // The motivating case: Amazon-scraped ComicInfo carries the
        // verbose `Stillwater by Zdarsky & Pérez` form. The `&` marks
        // it as a multi-author list; strip cleanly so token-set
        // similarity matches the catalog's terse `Stillwater`.
        assert_eq!(
            normalize_title("Stillwater by Zdarsky & Pérez"),
            "stillwater"
        );
    }

    #[test]
    fn strips_trailing_by_attribution_with_comma_or_and() {
        // Comma-separated and ` and ` author lists are the other
        // multi-author hallmarks the strip recognizes.
        assert_eq!(normalize_title("Foo by Author One, Author Two"), "foo");
        assert_eq!(
            normalize_title("Bar by Brian K Vaughan and Pia Guerra"),
            "bar"
        );
    }

    #[test]
    fn preserves_title_internal_by_without_attribution_markers() {
        // The four live-catalog cases that previously got mangled by
        // an over-aggressive strip. Each `by` is part of the title
        // itself; no `&`, `,`, or ` and ` in the suffix → leave it.
        assert_eq!(
            normalize_title("Step By Bloody Step"),
            "step by bloody step"
        );
        assert_eq!(
            normalize_title("The Nice House by the Sea"),
            "nice house by the sea"
        );
        assert_eq!(normalize_title("Touched by a Demon"), "touched by a demon");
        assert_eq!(
            normalize_title("Door to Door, Night by Night"),
            "door to door night by night"
        );
    }

    #[test]
    fn does_not_strip_single_author_by_attribution() {
        // Conservative trade-off: `Series by Brian Wood` has no `&`
        // / `,` / ` and ` marker so we can't distinguish it from
        // `title by Title-Internal Words`. Leave it untouched.
        // Documented expected behavior, not aspirational.
        assert_eq!(
            normalize_title("Series by Brian Wood"),
            "series by brian wood"
        );
    }

    #[test]
    fn does_not_strip_by_when_it_is_the_whole_title() {
        // A hypothetical one-word title `By` (or any leading-`by`
        // edge case) must survive — `head.trim().is_empty()` guard.
        assert_eq!(normalize_title("by"), "by");
    }

    #[test]
    fn numeric_content_preserved() {
        assert_eq!(normalize_title("100 Bullets"), "100 bullets");
        assert_eq!(normalize_title("52"), "52");
    }

    #[test]
    fn strips_only_first_article() {
        // "the a team" → strip "the " → "a team", we do NOT then strip "a "
        assert_eq!(normalize_title("The A Team"), "a team");
    }

    #[test]
    fn round_trip_already_normalized() {
        let normalized = normalize_title("Walking Dead");
        assert_eq!(normalize_title(&normalized), normalized);
    }

    #[test]
    fn matches_across_punctuation_styles() {
        // The point of normalization: equivalent inputs map to the same string.
        assert_eq!(
            normalize_title("Spider-Man: Far From Home"),
            normalize_title("spider-man far from home")
        );
        assert_eq!(normalize_title("X-Men"), normalize_title("the x-men"));
    }

    #[test]
    fn handles_parens_and_brackets() {
        assert_eq!(normalize_title("Saga (2012)"), "saga 2012");
        assert_eq!(normalize_title("Saga [Vol 1]"), "saga vol 1");
    }
}
