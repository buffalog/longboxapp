// Release calendar API — ComicVine on-sale dates for a date range,
// cache-backed server-side.
import { apiFetch } from './client';

/** One release-calendar row. Source-unified: a CV-sourced row (current /
 *  past weeks) populates `cv_issue_id` + `cv_volume_id` and leaves
 *  `metron_issue_id = null`; a Metron-sourced row (forward weeks)
 *  always populates `metron_issue_id` and populates `cv_*` only when
 *  Metron carries those cross-references inline.
 *
 *  At least one of `cv_issue_id` or `metron_issue_id` is always
 *  populated — the page's keyed-each uses
 *  `row.cv_issue_id ?? row.metron_issue_id` as the unique key. */
export interface CalendarRow {
  cv_issue_id: number | null;
  cv_volume_id: number | null;
  /** Metron's issue id (Item A v2). Populated only on forward-week
   *  rows; null on every CV-sourced row. */
  metron_issue_id: number | null;
  issue_number: string;
  /** On-sale date, `YYYY-MM-DD`. */
  store_date: string;
  volume_name: string;
  cover_url: string | null;
  site_detail_url: string;
  /** The tracked series this volume maps to, if LongBox knows it. */
  series_id: number | null;
  on_pull_list: boolean;
  /** Publisher name. CV-sourced rows chain series JOIN -> cv_volume_cache
   *  -> null. Metron-sourced rows chain series JOIN (via cv_id or
   *  metron_id) -> Metron-inline -> null. Frontend groups null rows
   *  under "Unknown Publisher". */
  publisher: string | null;
}

/** Fetch the release calendar for `[from, to]` (both `YYYY-MM-DD`).
 *  `refresh` forces a ComicVine re-query past the server-side cache. */
export function getReleaseCalendar(
  from: string,
  to: string,
  refresh = false
): Promise<CalendarRow[]> {
  const params = new URLSearchParams({ from, to });
  if (refresh) params.set('refresh', 'true');
  return apiFetch(`/releases/calendar?${params.toString()}`);
}

/** Compound "add to pull list": creates the series from ComicVine when
 *  LongBox doesn't track the volume yet, then subscribes it. Idempotent. */
export function addCalendarVolumeToPullList(
  cvVolumeId: number
): Promise<{ series_id: number }> {
  return apiFetch('/releases/calendar/pull', {
    method: 'POST',
    body: JSON.stringify({ cv_volume_id: cvVolumeId })
  });
}

/** Per-volume outcome of a bulk add-to-pull-list. */
export interface BulkAddResult {
  cv_volume_id: number;
  status: 'added' | 'already_on_list' | 'failed';
  series_id?: number;
  error?: string;
}

/** Bulk "add to pull list" — non-transactional; one result per volume,
 *  each with a 3-way status the caller tallies into a single toast. */
export function bulkAddCalendarVolumesToPullList(
  cvVolumeIds: number[]
): Promise<{ results: BulkAddResult[] }> {
  return apiFetch('/releases/calendar/pull/bulk', {
    method: 'POST',
    body: JSON.stringify({ cv_volume_ids: cvVolumeIds })
  });
}

/** One "release of note" — a volume on sale this ship-week whose name
 *  matches a series the user owns and that isn't on the pull list. */
export interface ReleaseOfNote {
  cv_volume_id: number;
  volume_name: string;
  cover_url: string | null;
  site_detail_url: string;
  issue_count: number;
}

/** This ship-week's "releases of note" — the dashboard discovery widget.
 *  The server computes the Wed–Tue range. */
export function getReleasesOfNote(): Promise<ReleaseOfNote[]> {
  return apiFetch('/releases/of-note');
}

/** One issue shipping this ship-week for a series on the pull list. */
export interface PullThisWeek {
  cv_issue_id: number;
  issue_number: string;
  store_date: string;
  cv_volume_id: number;
  volume_name: string;
  cover_url: string | null;
  site_detail_url: string;
}

/** This ship-week's calendar issues whose volume is on the pull list —
 *  the "This week's pulls" dashboard widget. */
export function getThisWeeksPulls(): Promise<PullThisWeek[]> {
  return apiFetch('/releases/this-weeks-pulls');
}
