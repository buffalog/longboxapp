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
