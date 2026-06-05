// API response types mirroring the longbox-web JSON shapes. Manually kept
// in sync with backend serde-Serialize output. Backend integration tests
// assert on response bodies; if those tests pass, the shapes here should
// hold. Phase B can switch to OpenAPI codegen if drift becomes painful.

export type FileStatus = 'owned' | 'needs_review' | 'unmatched' | 'ignored';

export type MatchMethod =
  | 'web_url_cv'
  | 'web_url_metron'
  | 'comicinfo_xml'
  | 'filename_regex'
  | 'manual'
  | 'phase_b'
  | 'unmatched'
  | 'ignored';

export type ScanKind = 'full' | 'rescan_unmatched' | 'rematch_for_series';

export interface Series {
  id: number;
  cv_id: number | null;
  metron_id: string | null;
  title: string;
  sort_title: string;
  start_year: number | null;
  publisher: string | null;
  description: string | null;
  cover_url: string | null;
  created_at: string;
  updated_at: string;
}

export interface SeriesWithCounts extends Series {
  total_count: number;
  owned_count: number;
  needs_review_count: number;
  ignored_count: number;
  unmatched_count: number;
  missing_count: number;
}

export interface Issue {
  id: number;
  series_id: number;
  cv_issue_id: number | null;
  metron_issue_id: string | null;
  number: string;
  title: string | null;
  cover_date: string | null;
  summary: string | null;
  cover_url: string | null;
  created_at: string;
  updated_at: string;
}

export interface FileSummary {
  id: number;
  path_relative: string;
  status: FileStatus;
  is_present: boolean;
}

export interface IssueWithFile extends Issue {
  file: FileSummary | null;
}

export interface SeriesDetail extends Series {
  issues: IssueWithFile[];
}

export interface FileRow {
  id: number;
  issue_id: number | null;
  library_root_id: number;
  path_relative: string;
  size_bytes: number;
  mtime: string;
  last_scanned_at: string;
  match_method: MatchMethod;
  match_confidence: number;
  status: FileStatus;
  cached_comicinfo_xml: string | null;
  cached_at: string | null;
  is_present: boolean;
  last_seen_at: string;
}

export interface IssueSnippet {
  id: number;
  number: string;
  title: string | null;
  cover_date: string | null;
}

export interface SeriesSnippet {
  id: number;
  title: string;
  start_year: number | null;
}

export interface EnrichedFileRow extends FileRow {
  issue: IssueSnippet | null;
  series: SeriesSnippet | null;
}

/** A row from the persisted `scan_runs` table. Returned by
 *  `/api/scans/recent` (filtered to user-triggered kinds). Phase A.5
 *  Task C: this replaced the in-memory `ScanReport` shape returned by
 *  the prior endpoint. Per-file error list and added/updated/missing
 *  counts that were on the in-memory shape are not persisted; the
 *  `files_added`/`files_updated` columns survive on the row but per-file
 *  errors live only in logs now. */
export interface ScanRun {
  id: number;
  library_root_id: number;
  started_at: string;
  finished_at: string | null;
  files_seen: number;
  files_added: number;
  files_updated: number;
  files_matched: number;
  files_needs_review: number;
  files_unmatched: number;
  status: 'running' | 'completed' | 'failed';
  error_message: string | null;
  kind: 'full' | 'rescan_unmatched' | 'rematch_for_series';
}

export interface CurrentScan {
  scan_id: string;
  library_root_id: number;
  kind: ScanKind;
  started_at: string;
}

export interface SeriesSearchResult {
  cv_id: number;
  name: string;
  start_year: number | null;
  publisher: string | null;
  issue_count: number;
  cover_url: string | null;
  description: string | null;
}

export interface Stats {
  total_series: number;
  total_issues: number;
  owned_files: number;
  needs_review_files: number;
  ignored_files: number;
  unmatched_files: number;
  /** Issues for which no present owned file exists. Not derivable from
   *  `total_issues - owned_files` because that conflates with
   *  needs_review and ignored states. */
  missing_issues: number;
  /** Distinct series with at least one missing issue. Always
   *  `<= total_series`. */
  series_with_missing: number;
  /** Series on the pull list. Powers the dashboard "Pull list" tile. */
  pull_list_count: number;
  /** Issues whose most recent pull attempt failed. Combined with
   *  `pending_interventions_count` to render the dashboard's
   *  "Needs attention" tile. */
  pull_failures_count: number;
  /** Files stuck in the post-processor's pending-intervention cache. */
  pending_interventions_count: number;
}

export interface StartScanResponse {
  scan_id: string;
  status: 'started';
}

export interface LibraryRoot {
  id: number;
  path: string;
  created_at: string;
}

// Phase B pending-intervention surface. Backed by an in-memory cache in
// longbox-postprocess; mirrors the InterventionReason serde shape
// (`#[serde(tag = "kind", content = "detail")]`).

export type InterventionReason =
  | { kind: 'conflict' }
  | { kind: 'comic_info_write_failed'; detail: string }
  | { kind: 'move_failed'; detail: string };

export interface PendingIntervention {
  source_path: string;
  target_path: string;
  reason: InterventionReason;
  size: number;
  last_attempt: string;
}

export interface PendingResponse {
  count: number;
  items: PendingIntervention[];
}
