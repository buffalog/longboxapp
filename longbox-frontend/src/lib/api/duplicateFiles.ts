import { apiFetch } from './client';

export interface DupCandidate {
  file_id: number;
  path_relative: string;
  size_bytes: number;
  format: string;
  parsed_number: string | null;
  is_served: boolean;
  suspect_corrupt: boolean;
  under_unsorted: boolean;
}

export interface DupGroup {
  issue_id: number;
  series_title: string;
  issue_number: string;
  kind: 'duplicate' | 'mismatch';
  suggested_keep_file_id: number | null;
  files: DupCandidate[];
}

export interface DuplicateFilesPage {
  groups: DupGroup[];
  total: number;
  page: number;
  per_page: number;
}

export function listDuplicateFiles(page = 1, perPage = 50): Promise<DuplicateFilesPage> {
  const p = new URLSearchParams({ page: String(page), per_page: String(perPage) });
  return apiFetch(`/library/tidy/duplicate-files?${p.toString()}`);
}

export interface Resolution {
  issue_id: number;
  keep_file_id: number;
}

export interface ResolveResult {
  issue_id: number;
  status: 'resolved' | 'refused';
  kept_file_id: number | null;
  deleted_file_ids: number[];
  failed: { file_id: number; error: string }[];
  reason: string | null;
}

export function resolveDuplicateFiles(
  resolutions: Resolution[]
): Promise<{ results: ResolveResult[] }> {
  return apiFetch('/library/tidy/duplicate-files/resolve', {
    method: 'POST',
    body: JSON.stringify({ resolutions })
  });
}
