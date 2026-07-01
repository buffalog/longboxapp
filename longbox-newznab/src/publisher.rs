//! Static curated publisher/imprint token list. Scene/Usenet release names
//! routinely prepend the publisher or imprint as a literal token
//! ("Vertigo-Fbp...", "DC.K.O..."), and some catalog titles carry it too
//! ("DC K.O.", "EC Cruel Universe"). The match filter strips these tokens
//! from BOTH the requested title and the parsed release title so an
//! unrequested imprint token doesn't dilute similarity.
//!
//! Deliberately a static list, not derived from `series.publisher`: that
//! column is parent-level ("DC Comics") not scene-level ("Vertigo"), and
//! using real data as a token source reintroduces the fragile-heuristic
//! problem the similarity filter already has. Missing imprint → one-line
//! addition here when it surfaces (see GH #12 follow-up rationale).

/// Lowercase, single-word imprint/publisher tokens tolerated as noise.
/// Multi-word publishers are listed token-by-token (e.g. "dark", "horse").
/// Keep entries lowercase — matching happens in normalized (lowercased) space.
const PUBLISHER_TOKENS: &[&str] = &[
    "marvel",
    "dc",
    "image",
    "vertigo",
    "darkhorse",
    "madcave",
    "boom",
    "idw",
    "dynamite",
    "aftershock",
    "ablaze",
    "ec",
    "dstlry",
    "skybound",
    "wildstorm",
    "milestone",
    "blackmask",
];

/// Remove leading/trailing publisher tokens from a normalized,
/// space-separated token string. Only strips tokens at the EDGES of the
/// string so an internal word that happens to collide (rare) is preserved;
/// scene names put the imprint at the front, occasionally the back.
/// Never strips the last remaining token (so a title that IS just a
/// publisher word can't normalize to empty).
///
/// SAFETY BOUND: edge-stripping a token that is also a legitimate title word
/// is self-cancelling for the primary match (it's stripped from BOTH the
/// requested title and the release title), but it CAN collide two distinct
/// series whose stripped forms coincide. That residual false-positive is
/// bounded by the downstream exact issue-number gate and year gate in
/// `filter_by_series_title`. Do not reuse this helper in a matching context
/// that lacks those gates, and keep the token list to unambiguous
/// publisher/imprint names (no bare common-word halves of multi-word imprints).
pub fn strip_publisher_tokens(normalized: &str) -> String {
    let mut tokens: Vec<&str> = normalized.split_whitespace().collect();
    // strip from front
    while tokens.len() > 1 && PUBLISHER_TOKENS.contains(&tokens[0]) {
        tokens.remove(0);
    }
    // strip from back
    while tokens.len() > 1 && PUBLISHER_TOKENS.contains(tokens.last().unwrap()) {
        tokens.pop();
    }
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_imprint_token() {
        assert_eq!(
            strip_publisher_tokens("vertigo fbp federal bureau of physics"),
            "fbp federal bureau of physics"
        );
    }

    #[test]
    fn strips_leading_publisher_from_catalog_title() {
        // "DC K.O." normalized is "dc k o"; "EC Cruel Universe" -> "ec cruel universe".
        assert_eq!(strip_publisher_tokens("dc k o"), "k o");
        assert_eq!(
            strip_publisher_tokens("ec cruel universe"),
            "cruel universe"
        );
    }

    #[test]
    fn preserves_internal_and_non_publisher_tokens() {
        assert_eq!(strip_publisher_tokens("saga"), "saga");
        assert_eq!(
            strip_publisher_tokens("federal bureau of physics"),
            "federal bureau of physics"
        );
    }

    #[test]
    fn never_strips_to_empty() {
        // A title that is only a publisher word survives intact.
        assert_eq!(strip_publisher_tokens("vertigo"), "vertigo");
    }
}
