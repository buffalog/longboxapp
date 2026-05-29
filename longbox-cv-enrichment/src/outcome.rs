//! Per-attempt outcome values + diagnostic strings.
//!
//! The string forms are what land in `series.last_enrichment_outcome`
//! and are also what aggregate-summary endpoints filter on. The
//! enum is the worker-internal handoff between
//! `pick_volume → cv_id_collision check → fetch → merge → outcome record`.

use std::fmt;

/// Worker-internal outcome. Maps 1-to-1 to the `last_enrichment_outcome`
/// TEXT column (open enum per the 6c.1 migration comment).
#[derive(Debug, Clone, PartialEq)]
pub enum AttemptOutcome {
    /// Auto-pick succeeded, CV data fetched, all CV-issue numbers
    /// merged, no catalog-only numbers stranded.
    Matched { cv_id: i64, score: f64 },
    /// Auto-pick succeeded and the merge completed, but ≥ 1
    /// catalog issue number didn't appear in CV's issue list.
    /// `orphan_numbers` carries the stranded set for the
    /// structured log + the diagnostic written to
    /// `last_enrichment_error`.
    PartialMerge {
        cv_id: i64,
        score: f64,
        orphan_count: usize,
        orphan_numbers: Vec<String>,
    },
    /// CV search returned 0 candidates.
    NoResults,
    /// Best candidate scored below the title-similarity threshold.
    LowConfidence { best_score: f64 },
    /// Two or more candidates above threshold; dominant-gap guard
    /// refused.
    MultiMatch { best_score: f64, second_score: f64 },
    /// Sole above-threshold survivor failed the catalog start_year
    /// gate.
    YearMismatch,
    /// Sole above-threshold survivor failed the issue-count window
    /// guard.
    CountMismatch,
    /// Pre-filter refused: year-unknown + catalog-title-collision.
    /// Recorded as an explicit positive outcome so 6c.3's bucketed
    /// report shows "N: collision_disabled" as a signal the
    /// pre-filter ran, not as absence.
    CollisionDisabled,
    /// `find_by_cv_id` showed another series already claims this
    /// cv_id. Records both ids in the diagnostic for the eventual
    /// series-merge prompt.
    CvIdCollision { cv_id: i64, other_series_id: i64 },
    /// `set_cv_id` UPDATE matched zero rows — another worker or
    /// manual user link claimed this series first. Treated as a
    /// real outcome (the race-guard predicate firing IS
    /// information), not silent success.
    SetCvIdRaceLost { cv_id: i64 },
    /// CV API or DB error during the attempt.
    Error { detail: String },
}

impl AttemptOutcome {
    /// String form for `series.last_enrichment_outcome`. Mirrors the
    /// open-enum vocabulary documented in the 6c.1 migration.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            AttemptOutcome::Matched { .. } => "matched",
            AttemptOutcome::PartialMerge { .. } => "partial_merge",
            AttemptOutcome::NoResults => "no_results",
            AttemptOutcome::LowConfidence { .. } => "low_confidence",
            AttemptOutcome::MultiMatch { .. } => "multi_match",
            AttemptOutcome::YearMismatch => "year_mismatch",
            AttemptOutcome::CountMismatch => "count_mismatch",
            AttemptOutcome::CollisionDisabled => "collision_disabled",
            AttemptOutcome::CvIdCollision { .. } => "cv_id_collision",
            AttemptOutcome::SetCvIdRaceLost { .. } => "set_cv_id_race_lost",
            AttemptOutcome::Error { .. } => "error",
        }
    }

    /// Optional diagnostic for `series.last_enrichment_error`.
    /// Pairs with the structured log event the worker also emits.
    pub fn diagnostic(&self) -> Option<String> {
        match self {
            AttemptOutcome::Matched { score, .. } => Some(format!("score={score:.3}")),
            AttemptOutcome::PartialMerge {
                orphan_count,
                orphan_numbers,
                ..
            } => Some(format!(
                "{orphan_count} catalog issue numbers absent from CV: {}",
                orphan_numbers.join(", ")
            )),
            AttemptOutcome::LowConfidence { best_score } => {
                Some(format!("best similarity {best_score:.3} below threshold"))
            }
            AttemptOutcome::MultiMatch {
                best_score,
                second_score,
            } => Some(format!(
                "two close candidates: {best_score:.3} vs {second_score:.3}"
            )),
            AttemptOutcome::CvIdCollision {
                cv_id,
                other_series_id,
            } => Some(format!(
                "series {other_series_id} already claims cv_id {cv_id}"
            )),
            AttemptOutcome::SetCvIdRaceLost { cv_id } => Some(format!(
                "another writer claimed cv_id {cv_id} for this series first"
            )),
            AttemptOutcome::Error { detail } => Some(detail.clone()),
            _ => None,
        }
    }
}

impl fmt::Display for AttemptOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.diagnostic() {
            Some(d) => write!(f, "{} ({d})", self.as_db_str()),
            None => write!(f, "{}", self.as_db_str()),
        }
    }
}
