// Release calendar API — ComicVine on-sale dates for a date range,
// cache-backed server-side.
import { apiFetch } from './client';

/** One release-calendar row: a CV issue plus live pull-list enrichment. */
export interface CalendarRow {
  cv_issue_id: number;
  issue_number: string;
  /** On-sale date, `YYYY-MM-DD`. */
  store_date: string;
  cv_volume_id: number;
  volume_name: string;
  cover_url: string | null;
  site_detail_url: string;
  /** The tracked series this volume maps to, if LongBox knows it. */
  series_id: number | null;
  on_pull_list: boolean;
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
