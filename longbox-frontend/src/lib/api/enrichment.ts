// CV-enrichment surface — the Library Tidy section that lets the user
// resolve series the auto-pick worker refused (multi-match / low-
// confidence / year-mismatch / collision-disabled / error). Two reads
// and one mutation.
import { apiFetch } from './client';
import type { Series } from '../types';

/** The shape `GET /api/library/tidy/enrichment-summary` returns —
 *  aggregate counts the dashboard chip and the Tidy header read. The
 *  per-outcome counts inside `recent_outcomes` are scoped to shallow
 *  series (`cv_id IS NULL`), so `matched`/`partial_merge` are
 *  effectively always zero — both transitions promote the series out
 *  of the shallow set. The counts that matter for the queue UI are
 *  the five review-required outcomes. */
export interface EnrichmentSummary {
  shallow_total: number;
  awaiting_attempt: number;
  cooldown_waiting: number;
  is_running: boolean;
  recent_outcomes: {
    matched: number;
    partial_merge: number;
    no_results: number;
    low_confidence: number;
    multi_match: number;
    year_mismatch: number;
    count_mismatch: number;
    collision_disabled: number;
    cv_id_collision: number;
    set_cv_id_race_lost: number;
    error: number;
  };
}

/** The five outcomes the enrichment-queue endpoint surfaces — each
 *  one corresponds to a different reason the auto-pick worker
 *  refused, and each gets its own badge in the Library Tidy section.
 *  Distinct from the broader set in [`EnrichmentSummary`] because the
 *  queue filters to the review-actionable subset only. */
export type EnrichmentReviewOutcome =
  | 'multi_match'
  | 'low_confidence'
  | 'year_mismatch'
  | 'collision_disabled'
  | 'error';

/** One row in the disambiguation queue. `last_enrichment_error`
 *  carries the diagnostic text the worker recorded — usually empty
 *  for low_confidence/year_mismatch (the outcome IS the diagnostic),
 *  populated for `error`. */
export interface EnrichmentQueueRow {
  id: number;
  title: string;
  start_year: number | null;
  last_enrichment_outcome: EnrichmentReviewOutcome;
  last_enrichment_error: string | null;
  owned_count: number;
}

export function getEnrichmentSummary(): Promise<EnrichmentSummary> {
  return apiFetch('/library/tidy/enrichment-summary');
}

export function getEnrichmentQueue(): Promise<EnrichmentQueueRow[]> {
  return apiFetch('/library/tidy/enrichment-queue');
}

/** Set the series's `cv_id` to a user-picked CV volume, then wipe
 *  + re-fetch issues from CV. Mirrors `POST /api/series` but for an
 *  EXISTING series (the row stays put, only the identity column +
 *  descriptive fields + issue list change). Returns the updated
 *  series row. */
export function setSeriesCvId(seriesId: number, cvId: number): Promise<Series> {
  return apiFetch(`/series/${seriesId}/cv-id`, {
    method: 'PATCH',
    body: JSON.stringify({ cv_id: cvId })
  });
}
