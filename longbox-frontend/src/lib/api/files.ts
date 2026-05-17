import { apiFetch } from './client';
import type { EnrichedFileRow, FileStatus } from '../types';

export interface FilesQuery {
  status?: FileStatus | 'all';
  library_root_id?: number;
}

/** `/api/files` always returns matched issue + series embedded per file
 *  (no opt-in flag). When `issue_id` is null both `issue` and `series` are
 *  null. */
export function listFiles(query: FilesQuery = {}): Promise<EnrichedFileRow[]> {
  const params = new URLSearchParams();
  if (query.status) params.set('status', query.status);
  if (query.library_root_id !== undefined)
    params.set('library_root_id', String(query.library_root_id));
  const qs = params.toString();
  return apiFetch(`/files${qs ? '?' + qs : ''}`);
}

export function getFile(id: number): Promise<EnrichedFileRow> {
  return apiFetch(`/files/${id}`);
}

export function setFileIssue(id: number, issueId: number): Promise<EnrichedFileRow> {
  return apiFetch(`/files/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ issue_id: issueId })
  });
}

export function markFileIgnored(id: number): Promise<EnrichedFileRow> {
  return apiFetch(`/files/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ status: 'ignored' })
  });
}

export function clearFileIgnored(id: number): Promise<EnrichedFileRow> {
  return apiFetch(`/files/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ status: null })
  });
}

/** POST /api/files/:id/match-from-cv. Adds the CV volume to the watchlist
 *  (or reuses an existing series), resolves the issue number, and marks
 *  the file as manually owned. Throws ApiError with code
 *  `unprocessable.issue_number_unresolved` or
 *  `unprocessable.issue_not_in_series` when the match can't complete. */
export function matchFileFromCv(
  id: number,
  cvVolumeId: number,
  issueNumber?: string
): Promise<EnrichedFileRow> {
  const body: { cv_volume_id: number; issue_number?: string } = {
    cv_volume_id: cvVolumeId
  };
  if (issueNumber !== undefined && issueNumber.trim() !== '') {
    body.issue_number = issueNumber.trim();
  }
  return apiFetch(`/files/${id}/match-from-cv`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}
