//! Shallow-series CV enrichment auto-pick (Step 6c.1).
//!
//! Pure logic. Takes a catalog series's state + a pool of CV search
//! results + the configured thresholds, returns a single decision:
//! match a specific cv_id, refuse with a categorized outcome, or
//! surface a pre-filter disablement.
//!
//! Wrong auto-link is asymmetrically worse than refusal — a wrong
//! link overwrites cv_id + title + cover_date + summary + cover_url
//! on every merged issue, after which the series looks confidently
//! correct while being wrong, and the user has no reason to
//! re-check it. A refusal sits visibly in `/library/tidy` queue
//! waiting for human pick. Two independent nets reflect that
//! asymmetry:
//!
//! 1. **Pre-filter (knowable from catalog state alone).** Year-
//!    unknown catalog rows whose `sort_title` has ≥ 2 catalog
//!    siblings are routed straight to manual — never attempted CV.
//!    Cheap, certain, removes the highest false-positive surface.
//! 2. **Runtime dominant-gap guard (kicks in once CV responds).**
//!    Even if the title-similarity threshold + year gate + count
//!    gate all pass, two close-scoring candidates trip a refusal
//!    via the dominant-gap check. Same refuse-when-ambiguous
//!    discipline as Bug 2's phase-2 multi-match guard and Bug 3's
//!    sibling-MAX boundary.
//!
//! Decision sequence (top-of-file overview; specifics in
//! [`pick_volume`]):
//!
//! 1. Catalog-collision pre-filter (year-unknown only)
//! 2. Title-similarity gate (0.85 year-known / 0.95 year-unknown)
//! 3. Year gate (catalog with start_year requires exact match)
//! 4. Issue-count window gate (±20% year-known / ±10% year-unknown)
//! 5. Survivor count → dominant-gap guard

use longbox_core::normalize_title;
use longbox_core::similarity::similarity;

use crate::projection::SeriesSearchResult;

/// Input to [`pick_volume`]. The caller (the enrichment worker)
/// assembles this from the catalog row plus the catalog-collision
/// pre-check.
#[derive(Debug, Clone)]
pub struct EnrichmentCandidateInput {
    pub catalog_title: String,
    pub catalog_start_year: Option<i32>,
    pub catalog_issue_count: u32,
    /// `true` when ≥ 2 catalog rows share this series's normalized
    /// title (the `sort_title` collision check). Used only when
    /// `catalog_start_year` is `None` — for year-known catalog
    /// rows, the year gate already disambiguates same-titled
    /// candidates.
    pub catalog_title_collision: bool,
}

/// Tunable thresholds. The defaults match the
/// `settings.cv_enrichment_*` initial values; the worker reads
/// them at runtime via `settings_repo::get_or_default` so a value
/// change tunes the next cycle without restart.
#[derive(Debug, Clone, Copy)]
pub struct EnrichmentThresholds {
    pub title_threshold_year_known: f64,
    pub title_threshold_year_unknown: f64,
    pub count_window_year_known: f64,
    pub count_window_year_unknown: f64,
    pub dominant_gap: f64,
}

impl Default for EnrichmentThresholds {
    fn default() -> Self {
        Self {
            title_threshold_year_known: 0.85,
            title_threshold_year_unknown: 0.95,
            count_window_year_known: 0.20,
            count_window_year_unknown: 0.10,
            dominant_gap: 0.20,
        }
    }
}

/// Auto-pick decision. The worker maps this to a
/// `series.last_enrichment_outcome` value and, for `Matched`,
/// proceeds to fetch + merge (after the `find_by_cv_id`
/// pre-check for cv_id collision, which lives in the worker, not
/// here).
#[derive(Debug, Clone, PartialEq)]
pub enum PickOutcome {
    /// Auto-pick succeeded. `cv_id` is the CV volume to fetch and
    /// merge. The worker MUST still call
    /// `series_repo::find_by_cv_id(cv_id)` before assignment to
    /// catch the cv_id_collision case where another catalog
    /// series already claims this id.
    Matched { cv_id: i64, score: f64 },
    /// CV search returned 0 candidates.
    NoResults,
    /// Best candidate scored below the applicable title-similarity
    /// threshold.
    LowConfidence { best_score: f64 },
    /// Two or more candidates above all gates; the dominant-gap
    /// guard refused.
    MultiMatch { best_score: f64, second_score: f64 },
    /// Sole above-threshold survivor failed the catalog
    /// `start_year` gate.
    YearMismatch,
    /// Sole above-threshold survivor failed the issue-count window
    /// guard.
    CountMismatch,
    /// Pre-filter refused: year-unknown catalog row has ≥ 2 same-
    /// titled siblings in the catalog. Never attempted CV.
    CollisionDisabled,
}

/// Score multiplier applied to candidates [`is_collection_volume`]
/// recognizes as TPBs / hardcovers / omnibuses / etc. Half-strength
/// is deliberately punishing: at the 0.85 year-known threshold a
/// raw-1.0 collection lands at 0.5 and is rejected outright, so the
/// original-series candidate (no penalty) always wins. A
/// collection-only pool (no original at all) lands as LowConfidence
/// and the user is asked to pick — wrong auto-link to a TPB is
/// asymmetrically worse than refusal, same logic as the rest of
/// this module's gates.
const COLLECTION_PENALTY: f64 = 0.5;

/// Detect whether a CV search result describes a collected edition
/// (TPB / hardcover / omnibus / collected edition) rather than an
/// original ongoing or mini-series. Returns the name of the
/// triggering signal so the log line attributes the penalty to a
/// specific pattern (debuggable when CV's data drifts).
///
/// Three signal classes:
///
///  1. **Name** contains a collection-type word as a whole word
///     (Omnibus, Compendium, Absolute, Deluxe Edition, HC, TPB).
///     Word-boundary check via [`contains_word`] so "tpbeach"
///     doesn't false-match and parenthesized forms like
///     "Saga (TPB)" do.
///  2. **Description** contains one of the collection terms
///     ("trade paperback", "hardcover", "omnibus", "collected
///     edition") — heavy signal regardless of issue count.
///  3. `issue_count == 1` AND description contains "collecting" —
///     catches CV's TPB boilerplate ("Trade paperback collecting
///     issues 1-6 of...") without unconditionally penalizing the
///     bare word "collecting", which a multi-issue volume
///     might use in passing.
fn is_collection_volume(r: &SeriesSearchResult) -> Option<&'static str> {
    let name_lower = r.name.to_lowercase();
    const NAME_TERMS: &[&str] = &[
        "omnibus",
        "compendium",
        "absolute",
        "deluxe edition",
        "hc",
        "tpb",
    ];
    for term in NAME_TERMS {
        if contains_word(&name_lower, term) {
            return Some("name");
        }
    }

    let desc_lower = match r.description.as_deref() {
        Some(d) => d.to_lowercase(),
        None => return None,
    };

    const DESC_TERMS: &[&str] = &[
        "trade paperback",
        "hardcover",
        "omnibus",
        "collected edition",
    ];
    for term in DESC_TERMS {
        if desc_lower.contains(term) {
            return Some("desc");
        }
    }

    if r.issue_count == 1 && desc_lower.contains("collecting") {
        return Some("single_issue_collecting");
    }

    None
}

/// Whole-word substring check. `needle` is considered a "word" hit
/// when each end is either the string boundary or a non-alphabetic
/// byte. Pure ASCII boundary logic — sufficient for the collection-
/// type tokens we check (all ASCII), and avoids pulling regex into
/// this hot path.
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(rel) = haystack[start..].find(needle) {
        let abs = start + rel;
        let left_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphabetic();
        let right = abs + needle.len();
        let right_ok = right == bytes.len() || !bytes[right].is_ascii_alphabetic();
        if left_ok && right_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Run the auto-pick discipline against a CV result pool.
pub fn pick_volume(
    candidate: &EnrichmentCandidateInput,
    pool: &[SeriesSearchResult],
    thresholds: EnrichmentThresholds,
) -> PickOutcome {
    // (1) Catalog-collision pre-filter. Disables auto-pick entirely
    //     on the smallest, highest-risk slice of the population
    //     (year-unknown + ≥ 2 catalog siblings sharing the
    //     normalized title). Empirically 3 series at kickoff time;
    //     manual-pick cost is trivially small.
    if candidate.catalog_start_year.is_none() && candidate.catalog_title_collision {
        return PickOutcome::CollisionDisabled;
    }

    if pool.is_empty() {
        return PickOutcome::NoResults;
    }

    let year_known = candidate.catalog_start_year.is_some();
    let title_threshold = if year_known {
        thresholds.title_threshold_year_known
    } else {
        thresholds.title_threshold_year_unknown
    };
    let count_window = if year_known {
        thresholds.count_window_year_known
    } else {
        thresholds.count_window_year_unknown
    };

    let requested_normalized = normalize_title(&candidate.catalog_title);

    // Score every candidate up front. Sort by score desc so the
    // "best dropped via gate X" diagnostic uses the highest-scoring
    // candidate's failure reason.
    let mut scored: Vec<(f64, &SeriesSearchResult)> = pool
        .iter()
        .map(|r| {
            let raw_score = similarity(&requested_normalized, &normalize_title(&r.name));
            let score = if let Some(signal) = is_collection_volume(r) {
                tracing::debug!(
                    target: "longbox_cv_enrichment",
                    cv_id = r.cv_id,
                    name = %r.name,
                    raw_score,
                    penalty = COLLECTION_PENALTY,
                    signal,
                    "cv_enrichment.collection_penalty"
                );
                raw_score * COLLECTION_PENALTY
            } else {
                raw_score
            };
            (score, r)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let best_pre_filter_score = scored.first().map(|s| s.0).unwrap_or(0.0);

    // Track which gate the highest-scoring above-threshold candidate
    // failed, for the empty-survivors diagnostic.
    let mut survivors: Vec<(f64, &SeriesSearchResult)> = Vec::new();
    let mut best_failed_gate: Option<FailedGate> = None;

    for (score, r) in &scored {
        if *score < title_threshold {
            // Sorted desc, so all remaining candidates also below.
            if survivors.is_empty() && best_failed_gate.is_none() {
                best_failed_gate = Some(FailedGate::Title);
            }
            break;
        }
        // Year gate.
        if let (Some(req_year), Some(cand_year)) = (candidate.catalog_start_year, r.start_year) {
            if req_year != cand_year {
                if survivors.is_empty() && best_failed_gate.is_none() {
                    best_failed_gate = Some(FailedGate::Year);
                }
                continue;
            }
        }
        // Issue-count window gate. Skip when catalog has 0 issues
        // (avoids divide-by-zero and is the right behavior — a
        // freshly-converted shallow with no parsed issues yet has
        // no signal to gate on).
        if candidate.catalog_issue_count > 0 {
            let ratio = (r.issue_count as f64 - candidate.catalog_issue_count as f64).abs()
                / candidate.catalog_issue_count as f64;
            if ratio > count_window {
                if survivors.is_empty() && best_failed_gate.is_none() {
                    best_failed_gate = Some(FailedGate::Count);
                }
                continue;
            }
        }
        survivors.push((*score, r));
    }

    match survivors.len() {
        0 => match best_failed_gate {
            Some(FailedGate::Year) => PickOutcome::YearMismatch,
            Some(FailedGate::Count) => PickOutcome::CountMismatch,
            _ => PickOutcome::LowConfidence {
                best_score: best_pre_filter_score,
            },
        },
        1 => PickOutcome::Matched {
            cv_id: survivors[0].1.cv_id,
            score: survivors[0].0,
        },
        _ => {
            // Dominant-gap guard.
            let best = survivors[0];
            let second = survivors[1];
            if best.0 - second.0 >= thresholds.dominant_gap {
                PickOutcome::Matched {
                    cv_id: best.1.cv_id,
                    score: best.0,
                }
            } else {
                PickOutcome::MultiMatch {
                    best_score: best.0,
                    second_score: second.0,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailedGate {
    Title,
    Year,
    Count,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(cv_id: i64, name: &str, year: Option<i32>, issue_count: u32) -> SeriesSearchResult {
        SeriesSearchResult {
            cv_id,
            name: name.into(),
            start_year: year,
            publisher: None,
            issue_count,
            cover_url: None,
            description: None,
        }
    }

    fn input(title: &str, year: Option<i32>, count: u32, collision: bool) -> EnrichmentCandidateInput {
        EnrichmentCandidateInput {
            catalog_title: title.into(),
            catalog_start_year: year,
            catalog_issue_count: count,
            catalog_title_collision: collision,
        }
    }

    // -------- Pre-filter (catalog-collision disablement) --------

    #[test]
    fn pre_filter_year_unknown_with_catalog_collision_returns_collision_disabled() {
        // Sex / Sex Criminals / Monstress at kickoff time — never
        // attempts CV regardless of what the pool would have looked
        // like.
        let pool = vec![result(101, "Sex", Some(2013), 23)];
        let r = pick_volume(
            &input("Sex", None, 12, true),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert_eq!(r, PickOutcome::CollisionDisabled);
    }

    #[test]
    fn pre_filter_does_not_fire_when_year_is_known_even_with_collision() {
        // Year-known + same title (e.g., the Darkness 761/877 case)
        // doesn't trigger the pre-filter — the year gate is the
        // primary disambiguator and the find_by_cv_id pre-check at
        // the worker level catches cv_id collisions. Catalog count
        // 5 / CV count 5 — within the ±20% gate. Will Match
        // (collision-disablement does not fire); the cv_id_collision
        // surface is the worker's responsibility, not pick_volume's.
        let pool = vec![result(50, "The Darkness", Some(2025), 5)];
        let r = pick_volume(
            &input("The Darkness", Some(2025), 5, true),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(matches!(r, PickOutcome::Matched { cv_id: 50, .. }));
    }

    // -------- NoResults --------

    #[test]
    fn empty_pool_is_no_results() {
        let r = pick_volume(
            &input("Wolverine", Some(1982), 4, false),
            &[],
            EnrichmentThresholds::default(),
        );
        assert_eq!(r, PickOutcome::NoResults);
    }

    // -------- Matched: clean cases --------

    #[test]
    fn clean_year_known_exact_match() {
        // Saga catalog with year + Image's CV Saga at same year.
        let pool = vec![result(123, "Saga", Some(2012), 72)];
        let r = pick_volume(
            &input("Saga", Some(2012), 72, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(matches!(r, PickOutcome::Matched { cv_id: 123, .. }));
    }

    #[test]
    fn clean_year_unknown_title_unique_match() {
        // Year-unknown shallow series with a unique-in-catalog title
        // and a single high-scoring CV candidate clears the 0.95
        // threshold and matches.
        let pool = vec![result(456, "Scalped", None, 60)];
        let r = pick_volume(
            &input("Scalped", None, 59, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(matches!(r, PickOutcome::Matched { cv_id: 456, .. }));
    }

    // -------- LowConfidence --------

    #[test]
    fn best_below_title_threshold_is_low_confidence() {
        // Substantially different titles: "Wolverine" vs "Beware the
        // Eye of Odin" — same kind of score (~0.25) Bug 3 verified
        // live. Below the 0.85 year-known threshold.
        let pool = vec![result(99, "Beware the Eye of Odin", Some(2022), 1)];
        let r = pick_volume(
            &input("Wolverine", Some(2022), 1, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        match r {
            PickOutcome::LowConfidence { best_score } => {
                assert!(best_score < 0.85, "got {best_score}");
            }
            other => panic!("expected LowConfidence, got {other:?}"),
        }
    }

    // -------- YearMismatch --------

    #[test]
    fn sole_above_threshold_with_wrong_year_is_year_mismatch() {
        // Catalog says 1982 Wolverine (the original mini); CV returns
        // a 2024 Wolverine relaunch with year=2024. Title passes
        // 0.85 (identical), year gate refuses.
        let pool = vec![result(700, "Wolverine", Some(2024), 4)];
        let r = pick_volume(
            &input("Wolverine", Some(1982), 4, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert_eq!(r, PickOutcome::YearMismatch);
    }

    // -------- CountMismatch --------

    #[test]
    fn sole_above_threshold_with_wrong_count_is_count_mismatch() {
        // Year-known with ±20% window. CV has 200 issues; catalog
        // has 50 — 150/50 = 300% off, well past the gate.
        let pool = vec![result(800, "Saga", Some(2012), 200)];
        let r = pick_volume(
            &input("Saga", Some(2012), 50, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert_eq!(r, PickOutcome::CountMismatch);
    }

    // -------- Threshold delta: year-known vs year-unknown --------

    /// "Wolverine" vs "Wolverine MAX" — Bug 3's calibration case.
    /// Similarity ≈ 0.69 — passes 0.65 (NEEDS_REVIEW_FLOOR) but
    /// fails 0.75 (Bug 3 threshold) and also fails 0.85
    /// (cv_enrichment year-known). Year-unknown raises the bar
    /// further to 0.95.
    #[test]
    fn sibling_series_fails_year_known_threshold() {
        let pool = vec![result(901, "Wolverine MAX", Some(2024), 1)];
        let r = pick_volume(
            &input("Wolverine", Some(2024), 1, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        // 0.69 < 0.85 → LowConfidence.
        match r {
            PickOutcome::LowConfidence { best_score } => {
                assert!(best_score < 0.85 && best_score > 0.65, "got {best_score}");
            }
            other => panic!("expected LowConfidence, got {other:?}"),
        }
    }

    #[test]
    fn year_unknown_threshold_is_strictly_tighter() {
        // A candidate scoring ~0.88 would pass year-known (0.85)
        // but should fail year-unknown (0.95). Build a fixture:
        // "Saga" (catalog) vs "Saga Vol" (CV) — Jaccard 1/2 = 0.5,
        // Levenshtein 4/7 = 0.571 (max 0.571). Too low. Try
        // "Saga" vs "Sagas" — Levenshtein 4/5 = 0.8, Jaccard 0
        // (different tokens). Max 0.8 — passes 0.85? No, 0.8 < 0.85.
        // Try "Saga" vs "Sagaa" — Levenshtein 4/5 = 0.8 again.
        // To get >= 0.85 < 0.95: need a slight-typo case. Use
        // "Saga" vs "Saga." — punctuation drops via normalize, both
        // become "saga", similarity 1.0. Not useful.
        // Use catalog "Walking Dead" vs CV "The Walking Dead" —
        // normalize drops "the" → both "walking dead" → 1.0.
        // Use "Wolverine" vs "Wolverin" — Levenshtein 8/9 = 0.889,
        // passes year-known (0.85), fails year-unknown (0.95).
        let pool = vec![result(902, "Wolverin", None, 5)];
        let r = pick_volume(
            &input("Wolverine", None, 5, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        match r {
            PickOutcome::LowConfidence { best_score } => {
                assert!((0.85..0.95).contains(&best_score), "got {best_score}");
            }
            other => panic!("expected LowConfidence at the year-unknown threshold, got {other:?}"),
        }
    }

    // -------- MultiMatch + dominant-gap guard --------

    #[test]
    fn two_close_above_threshold_candidates_trip_multi_match() {
        // Two equally-good "Wolverine"s — both exact title, both
        // 2024, both 20 issues. Dominant-gap is 0, < 0.20 → refuse.
        let pool = vec![
            result(1000, "Wolverine", Some(2024), 20),
            result(1001, "Wolverine", Some(2024), 20),
        ];
        let r = pick_volume(
            &input("Wolverine", Some(2024), 20, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(
            matches!(r, PickOutcome::MultiMatch { .. }),
            "got {r:?}"
        );
    }

    #[test]
    fn dominant_gap_lets_clear_winner_through_two_survivors() {
        // First candidate exact match (1.0), second is "The
        // Wolverine" — after normalize_title strips "the" → also
        // "wolverine" → also 1.0. Equal. So instead use a synthetic
        // case where best=1.0 and second=0.78 (below 0.85 in fact
        // — wouldn't even survive). To get two above-threshold:
        // best=1.0, second=0.85. That's a 0.15 gap, just below the
        // 0.20 dominant_gap → MultiMatch.
        // To get Matched via dominant-gap: best=1.0, second=0.79
        // wouldn't be a survivor (below 0.85). So having both
        // above-threshold WITH a 0.20+ gap is unusual. Construct
        // explicitly: best="Saga" 1.0, second="Saga Special" — has
        // tokens {saga, special} vs {saga} → Jaccard 0.5,
        // Levenshtein 7/12 = 0.417 → max 0.5. Not above threshold.
        // The empirical reality: candidates above 0.85 cluster
        // tightly. So dominant-gap-lets-through is rare. Still
        // worth a synthetic test: tweak thresholds for the test to
        // expose the path.
        let thresholds = EnrichmentThresholds {
            title_threshold_year_known: 0.50,
            dominant_gap: 0.20,
            ..EnrichmentThresholds::default()
        };
        let pool = vec![
            result(2000, "Wolverine", Some(2024), 5),
            // "Wolverine Annual" — similarity vs "Wolverine" ≈ 0.62.
            // Above the lowered 0.50 threshold, but the gap from 1.0
            // is 0.38 → dominant-gap guard accepts.
            result(2001, "Wolverine Annual", Some(2024), 5),
        ];
        let r = pick_volume(
            &input("Wolverine", Some(2024), 5, false),
            &pool,
            thresholds,
        );
        assert!(
            matches!(r, PickOutcome::Matched { cv_id: 2000, .. }),
            "expected dominant-gap accept of 2000, got {r:?}"
        );
    }

    // -------- Count window: year-known vs year-unknown --------

    #[test]
    fn count_window_year_known_accepts_18_percent_off() {
        // Invincible-shape: catalog 122, CV 144 — 22/122 = 18%,
        // inside the ±20% year-known window. (Year-unknown ±10%
        // would refuse — see next test.)
        let pool = vec![result(3000, "Invincible", Some(2003), 144)];
        let r = pick_volume(
            &input("Invincible", Some(2003), 122, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(matches!(r, PickOutcome::Matched { cv_id: 3000, .. }));
    }

    #[test]
    fn count_window_year_unknown_refuses_18_percent_off() {
        // Same shape with year-unknown — 18% > 10% window →
        // CountMismatch. This is the kickoff's "Invincible without
        // year" scenario; the tighter year-unknown window catches
        // it and routes to manual.
        let pool = vec![result(3001, "Invincible", None, 144)];
        let r = pick_volume(
            &input("Invincible", None, 122, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert_eq!(r, PickOutcome::CountMismatch);
    }

    #[test]
    fn count_gate_skipped_when_catalog_has_zero_issues() {
        // Freshly-converted shallow with no parsed issues yet (count=0).
        // Skip the count gate (no signal) — fall through on year +
        // title only.
        let pool = vec![result(4000, "Saga", Some(2012), 72)];
        let r = pick_volume(
            &input("Saga", Some(2012), 0, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(matches!(r, PickOutcome::Matched { cv_id: 4000, .. }));
    }

    // -------- Year gate skipped when candidate has no start_year --------

    #[test]
    fn year_gate_passes_when_candidate_has_no_year() {
        // CV occasionally returns volumes without start_year. The
        // gate is conservative: if EITHER side lacks year, skip.
        // Same precedent as Bug 3's year filter ("pass on absence").
        let pool = vec![result(5000, "Saga", None, 72)];
        let r = pick_volume(
            &input("Saga", Some(2012), 72, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(matches!(r, PickOutcome::Matched { cv_id: 5000, .. }));
    }

    // -------- Collection-type penalty (TPB / HC / Omnibus) --------

    fn result_with_desc(
        cv_id: i64,
        name: &str,
        year: Option<i32>,
        issue_count: u32,
        description: &str,
    ) -> SeriesSearchResult {
        SeriesSearchResult {
            cv_id,
            name: name.into(),
            start_year: year,
            publisher: None,
            issue_count,
            cover_url: None,
            description: Some(description.into()),
        }
    }

    /// The exact CV-boilerplate signal: a single-issue volume whose
    /// description starts with "Trade paperback collecting". The
    /// pre-fix Beneath-the-Trees mis-link rode this exact shape.
    #[test]
    fn tpb_description_penalized() {
        // Only candidate is the TPB; if the penalty is in effect it
        // drops below the 0.85 threshold and the call falls through
        // to LowConfidence rather than auto-linking the TPB.
        let pool = vec![result_with_desc(
            160379,
            "Beneath the Trees Where Nobody Sees",
            Some(2024),
            1,
            "Trade paperback collecting issues 1-6 of the smash hit horror series.",
        )];
        let r = pick_volume(
            &input("Beneath the Trees Where Nobody Sees", Some(2024), 6, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        // Raw score is 1.0; penalty halves it to 0.5, below the
        // 0.85 year-known threshold → LowConfidence.
        assert!(
            matches!(r, PickOutcome::LowConfidence { best_score } if (best_score - 0.5).abs() < 1e-9),
            "expected LowConfidence at 0.5 (1.0 raw * 0.5 penalty), got {r:?}"
        );
    }

    /// Name-class signal: "Omnibus" as a whole word in the volume
    /// name. Triggers regardless of description or issue count.
    #[test]
    fn omnibus_name_penalized() {
        let pool = vec![result(7000, "Saga Omnibus", Some(2020), 3)];
        let r = pick_volume(
            &input("Saga", Some(2020), 72, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        // "Saga Omnibus" vs catalog "Saga" already scores below
        // the year-known threshold on title alone — but the
        // penalty pushes it further from any future close-call.
        // The assertion that matters: the omnibus does not get
        // auto-linked even on a permissive count window.
        assert!(
            !matches!(r, PickOutcome::Matched { cv_id: 7000, .. }),
            "Omnibus must never auto-link, got {r:?}"
        );
    }

    /// Negative control: a normal series with a healthy issue count
    /// and no collection signals in name OR description must NOT be
    /// penalized.
    #[test]
    fn normal_series_unaffected() {
        // issue_count=6 fails the single-issue gate; description
        // lacks the boilerplate exact phrase; name has no triggers.
        let pool = vec![result_with_desc(
            154239,
            "Beneath the Trees Where Nobody Sees",
            Some(2024),
            6,
            "An ongoing horror series about a teddy bear.",
        )];
        let r = pick_volume(
            &input("Beneath the Trees Where Nobody Sees", Some(2024), 6, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(
            matches!(r, PickOutcome::Matched { cv_id: 154239, .. }),
            "normal series must auto-link without penalty, got {r:?}"
        );
    }

    /// Composed scenario: catalog matches the original 6-issue
    /// series's title, year, and count exactly. The CV pool returns
    /// BOTH the original AND the TPB at the same title similarity.
    /// Without the penalty the dominant-gap guard refuses (both at
    /// 1.0). With the penalty the original beats the TPB by 0.5,
    /// well past the dominant gap, and the auto-link picks the
    /// original — this is the load-bearing fix for the
    /// Beneath-the-Trees mis-link.
    #[test]
    fn original_beats_tpb_same_title() {
        let pool = vec![
            result_with_desc(
                154239,
                "Beneath the Trees Where Nobody Sees",
                Some(2024),
                6,
                "An ongoing horror series.",
            ),
            result_with_desc(
                160379,
                "Beneath the Trees Where Nobody Sees",
                Some(2024),
                1,
                "Trade paperback collecting issues 1-6.",
            ),
        ];
        let r = pick_volume(
            &input("Beneath the Trees Where Nobody Sees", Some(2024), 6, false),
            &pool,
            EnrichmentThresholds::default(),
        );
        assert!(
            matches!(r, PickOutcome::Matched { cv_id: 154239, .. }),
            "original must beat TPB, got {r:?}"
        );
    }

    /// contains_word: word boundary respected on both ends. Avoids
    /// false-firing collection name terms on bigger words while
    /// catching surrounding-punctuation forms.
    #[test]
    fn contains_word_respects_boundaries() {
        assert!(contains_word("saga tpb", "tpb"));
        assert!(contains_word("saga (tpb)", "tpb"));
        assert!(contains_word("saga (tpb) volume 1", "tpb"));
        assert!(contains_word("saga omnibus", "omnibus"));
        assert!(!contains_word("tpbeach", "tpb")); // substring, not word
        assert!(!contains_word("absolutely", "absolute")); // suffix, not word
        assert!(!contains_word("compendiumly", "compendium")); // synthetic but exercises right boundary
    }
}
