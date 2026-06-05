// Library Tidy reconciliation API — phantom series (catalog tracks a
// series, disk has no files) and untracked folders (disk has a
// series-shaped folder the catalog doesn't know).
import { apiFetch } from './client';

/** A zero-owned series — the `GET /reconcile/phantoms` row shape. */
export interface PhantomSeries {
  id: number;
  title: string;
  sort_title: string;
  start_year: number | null;
  publisher: string | null;
  cover_url: string | null;
  /** Owned+present file count at the last full scan. `> 0` marks a
   *  *transition* phantom — the series held files at the last scan and
   *  has lost them all since. */
  last_matched_count: number;
  /** True when the series is empty by intent — on the pull list or
   *  with a pull attempt, i.e. awaiting its first download rather than
   *  having lost files. */
  awaiting_first_download: boolean;
  /** Recovery deadline ("YYYY-MM-DD HH:MM:SS") when the series is
   *  marked for automatic removal; `null` when unmarked. */
  auto_tidy_due_at: string | null;
}

/** Both phantom surfaces. `all_zero_owned` is every zero-owned series;
 *  `with_transition` is its `last_matched_count > 0` subset — so the
 *  two lists OVERLAP. The `/library/tidy` page renders them as a
 *  disjoint partition instead (see its `$derived`). */
export interface PhantomsResponse {
  with_transition: PhantomSeries[];
  all_zero_owned: PhantomSeries[];
}

/** A discovered (untracked) top-level library folder. The list
 *  endpoint returns only rows where BOTH dismiss columns are NULL,
 *  so both fields are always `null` in practice — they're typed to
 *  match the wire shape. */
export interface DiscoveredFolder {
  id: number;
  folder_name: string;
  first_seen_at: string;
  last_seen_at: string;
  /** User-permanent dismiss (the Dismiss button). Preserved across
   *  re-detections. */
  dismissed_at: string | null;
  /** State-derived dismiss (post-add, post-convert, scanner F6).
   *  Cleared by the next re-detection of the folder as untracked. */
  auto_dismissed_at: string | null;
  file_count: number;
}

/** Lightweight reconciliation counts for the dashboard banner. */
export interface ReconcileCounts {
  phantoms_with_transition: number;
  untracked_folders: number;
}

/** One folder to resolve, for `addFolders`. */
export interface AddFolderInput {
  folder_name: string;
  cv_id: number;
}

/** `POST /reconcile/add` result — per-row best-effort outcomes. */
export interface AddResult {
  succeeded: { folder_name: string; series_id: number }[];
  failed: { folder_name: string; error: string }[];
}

/** `POST /reconcile/phantoms/bulk` result — per-row best-effort. */
export interface BulkDeleteResult {
  deleted: number[];
  skipped: { series_id: number; reason: string }[];
}

export function listPhantoms(): Promise<PhantomsResponse> {
  return apiFetch('/reconcile/phantoms');
}

export function listUntracked(): Promise<DiscoveredFolder[]> {
  return apiFetch('/reconcile/untracked');
}

/** Counts only — feeds the dashboard reconciliation banner without
 *  pulling the full phantom/untracked lists onto the landing page. */
export function getReconcileCounts(): Promise<ReconcileCounts> {
  return apiFetch('/reconcile/counts');
}

/** Resolve discovered folders against ComicVine. Per-row best-effort —
 *  inspect `AddResult.succeeded` / `.failed` rather than relying on a
 *  thrown error (a CV failure still resolves with HTTP 200). */
export function addFolders(folders: AddFolderInput[]): Promise<AddResult> {
  return apiFetch('/reconcile/add', {
    method: 'POST',
    body: JSON.stringify({ folders })
  });
}

/** Bulk-dismiss discovered folders. The count is rows *newly* dismissed. */
export function dismissFolders(folderNames: string[]): Promise<{ dismissed: number }> {
  return apiFetch('/reconcile/dismiss', {
    method: 'POST',
    body: JSON.stringify({ folder_names: folderNames })
  });
}

/** Strict single phantom delete — rejects with `ApiError` 404 (unknown
 *  series) or 409 (the series still owns files). */
export function deletePhantom(seriesId: number): Promise<{ deleted: number }> {
  return apiFetch(`/reconcile/phantom/${seriesId}`, { method: 'DELETE' });
}

/** Best-effort bulk phantom delete — see `BulkDeleteResult`. */
export function bulkDeletePhantoms(seriesIds: number[]): Promise<BulkDeleteResult> {
  return apiFetch('/reconcile/phantoms/bulk', {
    method: 'POST',
    body: JSON.stringify({ series_ids: seriesIds })
  });
}

/** "Keep" a transition phantom — reset `last_matched_count` to 0,
 *  demoting it from the transition surface to the steady-state list. */
export function keepPhantom(seriesId: number): Promise<{ kept: number }> {
  return apiFetch(`/reconcile/phantom/${seriesId}/keep`, { method: 'POST' });
}

/** Per-folder outcome of a bulk shallow-convert.
 *  - `added`  — the folder became a new tracked series.
 *  - `linked` — the folder matched an existing series on
 *               `(sort_title, start_year)` (A.9 hot-fix idempotency)
 *               and its files were attached to that survivor.
 *  - `failed` — the per-folder transaction rolled back. */
export interface ConvertResult {
  folder_name: string;
  status: 'added' | 'linked' | 'failed';
  series_id?: number;
  error?: string;
}

/** Bulk shallow-convert discovered folders into tracked series — no
 *  ComicVine. Each folder becomes a series with number-only issues
 *  synthesized from its filenames; its files attach as owned.
 *  Non-transactional, per-folder results. */
export function convertFolders(folderNames: string[]): Promise<{ results: ConvertResult[] }> {
  return apiFetch('/reconcile/convert', {
    method: 'POST',
    body: JSON.stringify({ folder_names: folderNames })
  });
}

/** One side of a duplicate-pair row. */
export interface DuplicatePair {
  /** Detection criterion that surfaced this pair. `same_cv_id` wins
   *  when both criteria match (defensive against UNIQUE-bypassed data). */
  kind: 'same_cv_id' | 'same_title_close_year';
  a_id: number;
  a_title: string;
  a_start_year: number | null;
  a_cv_id: number | null;
  a_owned_count: number;
  b_id: number;
  b_title: string;
  b_start_year: number | null;
  b_cv_id: number | null;
  b_owned_count: number;
}

export function listDuplicates(): Promise<{ pairs: DuplicatePair[] }> {
  return apiFetch('/library/tidy/duplicates');
}

/** Merge `source` into `target`. The caller decides which is which;
 *  by convention the higher owned-count side is the target. */
export function mergeDuplicates(
  targetSeriesId: number,
  sourceSeriesId: number
): Promise<{ target_series_id: number; source_series_id: number }> {
  return apiFetch('/library/tidy/duplicates/merge', {
    method: 'POST',
    body: JSON.stringify({
      target_series_id: targetSeriesId,
      source_series_id: sourceSeriesId
    })
  });
}
