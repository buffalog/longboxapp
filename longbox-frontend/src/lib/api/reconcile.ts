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
}

/** Both phantom surfaces. `all_zero_owned` is every zero-owned series;
 *  `with_transition` is its `last_matched_count > 0` subset — so the
 *  two lists OVERLAP. The `/library/tidy` page renders them as a
 *  disjoint partition instead (see its `$derived`). */
export interface PhantomsResponse {
  with_transition: PhantomSeries[];
  all_zero_owned: PhantomSeries[];
}

/** A discovered (untracked) top-level library folder. */
export interface DiscoveredFolder {
  id: number;
  folder_name: string;
  first_seen_at: string;
  last_seen_at: string;
  dismissed_at: string | null;
  file_count: number;
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
