//! Pure similarity scoring. Inputs are pre-normalized title strings.
//!
//! Two metrics are provided: token-set Jaccard (word-level set overlap,
//! order-insensitive) and normalized Levenshtein (character-level edit
//! distance scaled to `[0, 1]`). The matcher uses [`similarity`] which takes
//! the max of the two — Jaccard handles word reordering well, Levenshtein
//! handles minor typos and near-duplicates.

use std::collections::HashSet;

/// Combined similarity score in `[0.0, 1.0]`. Returns `max(jaccard, levenshtein)`.
pub fn similarity(a: &str, b: &str) -> f64 {
    jaccard_similarity(a, b).max(normalized_levenshtein(a, b))
}

/// Token-set Jaccard: split on whitespace into word sets, compute
/// `|A ∩ B| / |A ∪ B|`. Two empty strings score 1.0.
pub fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let set_a: HashSet<&str> = a.split_whitespace().collect();
    let set_b: HashSet<&str> = b.split_whitespace().collect();

    if set_a.is_empty() && set_b.is_empty() {
        return 1.0;
    }

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

/// Levenshtein edit distance normalized by the longer string's length.
/// Two empty strings score 1.0; an empty vs non-empty pair scores 0.0.
pub fn normalized_levenshtein(a: &str, b: &str) -> f64 {
    let len_a = a.chars().count();
    let len_b = b.chars().count();
    let max = len_a.max(len_b);

    if max == 0 {
        return 1.0;
    }

    let distance = levenshtein_distance(a, b);
    1.0 - (distance as f64 / max as f64)
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn jaccard_identical() {
        assert!(approx(jaccard_similarity("walking dead", "walking dead"), 1.0));
    }

    #[test]
    fn jaccard_disjoint() {
        assert!(approx(jaccard_similarity("walking dead", "iron man"), 0.0));
    }

    #[test]
    fn jaccard_partial_overlap() {
        // {"walking","dead"} vs {"walking","dead","extra"} = 2/3
        let s = jaccard_similarity("walking dead", "walking dead extra");
        assert!(approx(s, 2.0 / 3.0));
    }

    #[test]
    fn jaccard_token_order_insensitive() {
        assert!(approx(
            jaccard_similarity("dead walking", "walking dead"),
            1.0
        ));
    }

    #[test]
    fn jaccard_empty_pair() {
        assert!(approx(jaccard_similarity("", ""), 1.0));
    }

    #[test]
    fn jaccard_one_empty() {
        assert!(approx(jaccard_similarity("", "walking dead"), 0.0));
        assert!(approx(jaccard_similarity("walking dead", ""), 0.0));
    }

    #[test]
    fn levenshtein_identical() {
        assert!(approx(normalized_levenshtein("saga", "saga"), 1.0));
    }

    #[test]
    fn levenshtein_one_substitution() {
        // 1 edit / 4 chars = 0.75
        assert!(approx(normalized_levenshtein("saga", "sago"), 0.75));
    }

    #[test]
    fn levenshtein_completely_different() {
        // "abc" vs "xyz": 3 substitutions / 3 chars = 0.0
        assert!(approx(normalized_levenshtein("abc", "xyz"), 0.0));
    }

    #[test]
    fn levenshtein_empty_pair() {
        assert!(approx(normalized_levenshtein("", ""), 1.0));
    }

    #[test]
    fn levenshtein_one_empty() {
        assert!(approx(normalized_levenshtein("", "saga"), 0.0));
    }

    #[test]
    fn levenshtein_insertion() {
        // "saga" -> "sagas": 1 insertion / 5 chars = 0.8
        assert!(approx(normalized_levenshtein("saga", "sagas"), 0.8));
    }

    #[test]
    fn similarity_takes_max() {
        // Reordered words: Jaccard 1.0, Levenshtein much lower. similarity = 1.0.
        let s = similarity("walking dead the", "the walking dead");
        assert!(approx(s, 1.0));
    }

    #[test]
    fn similarity_typo_caught_by_levenshtein() {
        // "wlking dead" vs "walking dead": Jaccard 1/2 = 0.5 (different words),
        // Levenshtein (12 chars max, 1 edit) ~= 0.917. Max should be Levenshtein.
        let s = similarity("wlking dead", "walking dead");
        assert!(s > 0.9, "expected > 0.9, got {s}");
    }

    #[test]
    fn similarity_in_bounds() {
        for (a, b) in [
            ("", ""),
            ("a", ""),
            ("", "a"),
            ("saga", "saga"),
            ("the walking dead", "walking dead"),
            ("x-men", "uncanny x-men"),
        ] {
            let s = similarity(a, b);
            assert!(
                (0.0..=1.0).contains(&s),
                "similarity({a:?}, {b:?}) = {s} out of bounds"
            );
        }
    }
}
