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

export interface ScanReport {
  library_root_id: number;
  started_at: string;
  completed_at: string;
  duration_ms: number;
  files_seen: number;
  files_added: number;
  files_updated: number;
  files_marked_missing: number;
  matched_owned: number;
  matched_needs_review: number;
  matched_ignored: number;
  unmatched: number;
  errors: Array<{ path_relative: string; error_message: string }>;
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
