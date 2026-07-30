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
