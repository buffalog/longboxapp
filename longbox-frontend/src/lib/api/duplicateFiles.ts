import { apiFetch } from './client';

export interface DupCandidate {
  file_id: number;
  path_relative: string;
  size_bytes: number;
  format: string;
  parsed_number: string | null;
  /** Where this file should have been pointed, per its filename. Null when the
   * server couldn't produce an unambiguous suggestion — manual review only. */
  suggested_issue_id: number | null;
  is_served: boolean;
  suspect_corrupt: boolean;
  under_unsorted: boolean;
  /** The issue number this file's filename names, when the series has no
   * catalog record for it. An absence, not a confidence problem — the fix is
   * to refresh the series, not to make a judgement call. */
  missing_target_number: string | null;
}

export interface IssueOption {
  issue_id: number;
  number: string;
}

export interface DupGroup {
  issue_id: number;
  series_id: number;
  series_title: string;
  issue_number: string;
  /**
   * `duplicate` — same folder, byte-identical content; the only deletable kind.
   * `pending_analysis` — not content-analyzed yet, so nothing is known about
   * whether these are the same comic. Never deletable: absence of a hash is
   * not evidence of sameness or of difference.
   * `mismatch` — content differs AND the filenames claim different issues:
   * distinct issues wrongly merged onto one record, fixed by moving a stray.
   * `same_number_different_bytes` — content differs but every filename claims
   * the SAME issue. Usually one issue in two container formats (.cbr/.cbz) or
   * two encodings of it. Nothing to move, and not provably a duplicate either:
   * proving two containers hold the same comic needs page-level comparison,
   * which this feature does not do. Never deletable.
   * `cross_folder_*` — files span more than one series folder, so they are
   * never duplicates however well the numbers agree. `wrong_series` means the
   * folders name different series or different volume years; `same_series`
   * means one series stored under two folder spellings. Neither is deletable;
   * the distinction only changes the wording.
   */
  kind:
    | 'duplicate'
    | 'mismatch'
    | 'same_number_different_bytes'
    | 'pending_analysis'
    | 'cross_folder_same_series'
    | 'cross_folder_wrong_series';
  suggested_keep_file_id: number | null;
  /** Every issue in the series — the override list for a mismatch group. */
  issue_options: IssueOption[];
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

export interface CorrectResult {
  file_id: number;
  from_issue_id: number;
  to_issue_id: number;
}

/**
 * Re-point one mismatched file at its real issue. Nothing is deleted. The
 * server re-validates independently and 422s on any ambiguity, so a refusal
 * here is a real answer, not a UI bug.
 */
export function correctDuplicateFile(fileId: number, issueId: number): Promise<CorrectResult> {
  return apiFetch('/library/tidy/duplicate-files/correct', {
    method: 'POST',
    body: JSON.stringify({ file_id: fileId, issue_id: issueId })
  });
}
