import { apiFetch } from './client';

/** A pull-list entry for one series — the `GET /pull-list/:id` shape. */
export interface PullEntry {
  id: number;
  series_id: number;
  added_at: string;
  /** `null` = pull from the first solicited issue. */
  start_issue: string | null;
  paused: boolean;
  last_pull_attempt_at: string | null;
  last_successful_pull_at: string | null;
  /** Series-level consecutive-sweep-failure counter, zeroed on a
   *  successful pull. Informational only — not a parking threshold
   *  (issue-level parking lives in pull_attempts.retry_count). */
  failure_count: number;
}

/** A pull-list row joined with its series — the `GET /pull-list` shape. */
export interface PullListEntry {
  series_id: number;
  series_title: string;
  series_sort_title: string;
  series_start_year: number | null;
  paused: boolean;
  added_at: string;
  last_pull_attempt_at: string | null;
  last_successful_pull_at: string | null;
  failure_count: number;
}

export function listPullList(): Promise<PullListEntry[]> {
  return apiFetch('/pull-list');
}

/** Resolves to `null` when the series is not on the pull list. */
export function getPullEntry(seriesId: number): Promise<PullEntry | null> {
  return apiFetch(`/pull-list/${seriesId}`);
}

export function addToPullList(seriesId: number): Promise<PullEntry> {
  return apiFetch('/pull-list', {
    method: 'POST',
    body: JSON.stringify({ series_id: seriesId })
  });
}

export function setPullPaused(seriesId: number, paused: boolean): Promise<PullEntry> {
  return apiFetch(`/pull-list/${seriesId}`, {
    method: 'PATCH',
    body: JSON.stringify({ paused })
  });
}

export function removeFromPullList(seriesId: number): Promise<void> {
  return apiFetch(`/pull-list/${seriesId}`, { method: 'DELETE' });
}

/** Trigger an immediate pull sweep. Rejects with `ApiError` (409,
 *  `conflict.pull_running`) when a sweep is already in progress. */
export function checkPull(): Promise<void> {
  return apiFetch('/pull/check', { method: 'POST' });
}

/** Trigger an on-demand search for one series. Fire-and-forget — the
 *  server responds 202 immediately and runs the search in the
 *  background; the resulting submissions land on the pull list /
 *  needs-attention pages naturally. Rejects with `ApiError`:
 *   - 404 `not_found.pull_list entry` when the series isn't subscribed,
 *   - 409 `conflict.pull_search_running` when a search for that same
 *     series is already in flight (a daily sweep running concurrently
 *     does NOT block this — the per-series guard is independent). */
export function searchSeriesNow(seriesId: number): Promise<void> {
  return apiFetch(`/pull/search/${seriesId}`, { method: 'POST' });
}
