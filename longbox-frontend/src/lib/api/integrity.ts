import { apiFetch } from './client';

/** One file, with every piece of raw evidence the scan surfaces about it. */
export interface FileEvidence {
  file_id: number;
  path_relative: string;
  size_bytes: number;
  content_blake3: string | null;
  issue_id: number | null;
  issue_number: string | null;
  series_id: number | null;
  series_title: string | null;
  series_start_year: number | null;
  match_method: string;
  match_confidence: number;
  /** Identity the archive claims from its own internal paths. */
  archive_label: string | null;
  archive_label_kind: string | null;
  archive_issue: string | null;
  archive_series: string | null;
  comicinfo_number: string | null;
  comicinfo_series: string | null;
  /** Parsed from the filename with the production parser. */
  filename_issue: string | null;
}

export interface ContentDuplicateGroup {
  digest: string;
  size_bytes: number;
  redundant_bytes: number;
  distinct_issue_ids: number[];
  spans_multiple_series: boolean;
  files: FileEvidence[];
}

export type CrossFolderCategory = 'wrong_volume' | 'benign_variant' | 'trade_or_collection';

export interface CrossFolderFinding {
  file: FileEvidence;
  main_folder: string;
  actual_folder: string;
  category: CrossFolderCategory;
}

export interface FilenameDisagreement {
  file: FileEvidence;
  filename_says: string;
  bound_to: string;
}

export interface OrphanedOwnedRow {
  file_id: number;
  path_relative: string;
  match_method: string;
  is_present: boolean;
}

export interface WalkProvenance {
  root: string;
  files_seen: number;
  rows_compared: number;
  duration_ms: number;
  unreadable: string[];
}

export interface Reconciliation {
  provenance: WalkProvenance;
  orphans: string[];
  ghosts: string[];
  present_but_marked_absent: string[];
}

export interface Findings {
  reconciliation: Reconciliation;
  content_duplicates: ContentDuplicateGroup[];
  /** Size-colliding files with no digest yet. While this is non-zero the
   * content-duplicate count is a floor, not a total. */
  unanalyzed_candidates: number;
  cross_folder: CrossFolderFinding[];
  filename_disagreements: FilenameDisagreement[];
  orphaned_owned_rows: OrphanedOwnedRow[];
  /** file_id -> number of classes it appears in, only where > 1. */
  classes_per_file: Record<string, number>;
}

export interface HashStats {
  candidates: number;
  fresh: number;
  hashed: number;
  skipped: number;
  failed: number;
  bytes_hashed: number;
  labelled: number;
  first_failure: string | null;
}

export interface AnalyzeStatus {
  running: boolean;
  started_at: string | null;
  finished_at: string | null;
  last: HashStats | null;
  last_error: string | null;
}

export function getFindings(): Promise<Findings> {
  return apiFetch('/library/integrity/findings');
}

export function getAnalyzeStatus(): Promise<AnalyzeStatus> {
  return apiFetch('/library/integrity/analyze/status');
}

export function startAnalyze(): Promise<{ status: string }> {
  return apiFetch('/library/integrity/analyze', { method: 'POST' });
}

/** The walk produced a usable comparison, so a zero means "none found". */
export function walkIsConclusive(r: Reconciliation): boolean {
  return r.provenance.unreadable.length === 0 && r.provenance.files_seen > 0;
}

/**
 * Whether anything will search for a reverted issue again, and if not, why.
 *
 * Mirrors the server enum exactly. Every variant needs its own sentence: a
 * revert that will never be re-searched looks identical, from the outside, to
 * one that will, and "reverts to missing" on its own reads as "so it will come
 * back". On the current library that is false for roughly half the issues a
 * delete can touch, because 632 of 777 series are not on the pull list.
 */
export type SearchOutlook =
  | 'will_search'
  | 'series_not_on_pull_list'
  | 'series_paused'
  | 'below_series_start_issue'
  | 'no_usable_cover_date'
  | 'retries_exhausted'
  | 'grab_in_flight'
  | 'recently_submitted';

export interface RevertedIssue {
  issue_id: number;
  issue_number: string;
  series_title: string;
  /** The issue has no owned, present file any more. */
  now_missing: boolean;
  search_outlook: SearchOutlook;
}

export interface DeleteDuplicateResult {
  file_id: number;
  path_relative: string;
  /** `null` when the deleted copy was bound to no issue. */
  reverted: RevertedIssue | null;
  /** Verified copies of these bytes still on disk AFTER this delete. */
  remaining_in_group: number;
  /** Set when the row was removed but the bytes survived — now an orphan. */
  unlink_error: string | null;
}

export function deleteDuplicate(
  digest: string,
  fileId: number
): Promise<DeleteDuplicateResult> {
  return apiFetch('/library/integrity/duplicates/delete', {
    method: 'POST',
    body: JSON.stringify({ digest, file_id: fileId })
  });
}

/**
 * What actually happened, in the user's terms.
 *
 * Derived from the RESPONSE, never from the act of deleting. The single
 * live case that proves this matters: an issue can own a second file
 * outside the group entirely, so a successful delete leaves it owned.
 * Across the 37 live groups that is true once — invisible from inside
 * the group, which is why the server re-queries ownership and this reads
 * the answer rather than assuming it.
 */
export function describeRevert(r: RevertedIssue | null): string {
  if (!r) return 'That copy was not filed under any issue, so nothing changed in the catalog.';
  const who = `${r.series_title} #${r.issue_number}`;
  if (!r.now_missing) {
    return `${who} still owns another file, so it did not revert to missing.`;
  }
  const base = `${who} reverts to missing`;
  switch (r.search_outlook) {
    case 'will_search':
      return `${base} and will be searched on the next sweep.`;
    case 'series_not_on_pull_list':
      return `${base}. This series is not on the pull list, so nothing will search for it automatically.`;
    case 'series_paused':
      return `${base}. This series is paused on the pull list, so nothing will search for it until you resume it.`;
    case 'below_series_start_issue':
      return `${base}. It falls below this series' start issue, so the sweep skips it.`;
    case 'no_usable_cover_date':
      return `${base}. It has no usable cover date, so the sweep never treats it as a candidate.`;
    case 'retries_exhausted':
      return `${base}. Previous attempts failed too many times, so the sweep has stopped retrying it.`;
    case 'grab_in_flight':
      return `${base}. It has an in-flight grab; searching resumes after the next sweep clears it.`;
    case 'recently_submitted':
      return `${base}. It was submitted recently; searching resumes once that attempt ages out.`;
  }
}
