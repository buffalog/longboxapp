//! Title normalization. Applied at series-insert time to populate
//! `series.sort_title`, and at match time to the candidate ComicInfo `<Series>`
//! text or filename-parsed series text. Both sides of the comparison live in
//! the same normalized space.

/// Lowercase, strip a single leading "the"/"a"/"an" article, drop punctuation
/// except hyphens, collapse runs of whitespace.
pub fn normalize_title(input: &str) -> String {
    let lower = input.to_lowercase();
    let stripped = strip_leading_article(&lower);
    let cleaned = drop_punctuation(stripped);
    collapse_whitespace(&cleaned)
}

fn strip_leading_article(s: &str) -> &str {
    for prefix in ["the ", "a ", "an "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return rest;
        }
    }
    s
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
        assert_eq!(normalize_title("Theory of Everything"), "theory of everything");
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
