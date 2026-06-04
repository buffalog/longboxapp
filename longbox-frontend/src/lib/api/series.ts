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

/** Delete a series.
 *
 *  - `force=true` unlinks every file from this series's issues
 *    (`issue_id` to NULL, `status` to `needs_review`) and bypasses the
 *    owned-files guard. Used by the Library Tidy duplicate-cleanup
 *    flow — the unlinked files are kicked back into the unmatched
 *    pool so they re-match to their real series.
 *  - `deleteFiles=true` also removes the on-disk series folder
 *    (`{library_root}/{title} ({year})/`) via `remove_dir_all` after
 *    the DB delete. Incompatible with `force` — combining them is a
 *    400 from the server, since the force path implies the files are
 *    misassigned and the title-based folder belongs to ANOTHER series.
 *
 *  Response carries `folder_deleted` (the canonicalized path) when
 *  the disk-side removal actually ran. */
export function deleteSeries(
  id: number,
  opts: { force?: boolean; deleteFiles?: boolean } = {}
): Promise<{ deleted: number; folder_deleted?: string }> {
  const params = new URLSearchParams();
  if (opts.force) params.set('force', 'true');
  if (opts.deleteFiles) params.set('delete_files', 'true');
  const qs = params.toString();
  return apiFetch(`/series/${id}${qs ? `?${qs}` : ''}`, { method: 'DELETE' });
}

export function refreshSeries(id: number): Promise<Series> {
  return apiFetch(`/series/${id}/refresh`, { method: 'POST' });
}
