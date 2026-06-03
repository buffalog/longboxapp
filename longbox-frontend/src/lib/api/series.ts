import { apiFetch } from './client';
import type { Series, SeriesDetail, SeriesWithCounts } from '../types';

export function listSeries(): Promise<SeriesWithCounts[]> {
  return apiFetch('/series');
}

export function getSeries(id: number): Promise<SeriesDetail> {
  return apiFetch(`/series/${id}`);
}

export function addSeries(cvId: number): Promise<Series> {
  return apiFetch('/series', {
    method: 'POST',
    body: JSON.stringify({ cv_id: cvId })
  });
}

/** Delete a series. When `force` is true, the server unlinks every
 *  file from this series's issues — `issue_id` to NULL and `status` to
 *  `needs_review` — and bypasses the owned-files guard. The unlinked
 *  files are kicked back into the unmatched pool so they re-match to
 *  their real series (used by the Library Tidy duplicate-cleanup
 *  flow). */
export function deleteSeries(
  id: number,
  opts: { force?: boolean } = {}
): Promise<{ deleted: number }> {
  const qs = opts.force ? '?force=true' : '';
  return apiFetch(`/series/${id}${qs}`, { method: 'DELETE' });
}

export function refreshSeries(id: number): Promise<Series> {
  return apiFetch(`/series/${id}/refresh`, { method: 'POST' });
}
