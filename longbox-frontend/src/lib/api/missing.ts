import { apiFetch } from './client';

export interface MissingIssue {
  issue_id: number;
  number: string;
  title: string | null;
  cover_url: string | null;
  cover_date: string | null;
  issue_created_at: string;
  series: {
    id: number;
    title: string;
    sort_title: string;
    start_year: number | null;
  };
}

export interface MissingResponse {
  missing: MissingIssue[];
  total: number;
}

export type MissingSort = 'series' | 'cover_date';

export interface MissingQuery {
  series_id?: number;
  sort?: MissingSort;
}

export function getMissing(query: MissingQuery = {}): Promise<MissingResponse> {
  const params = new URLSearchParams();
  if (query.series_id !== undefined) params.set('series_id', String(query.series_id));
  if (query.sort) params.set('sort', query.sort);
  const qs = params.toString();
  return apiFetch(`/missing${qs ? '?' + qs : ''}`);
}

export interface SearchAllMissingSummary {
  /** Issues for which an indexer search was dispatched. */
  searched: number;
  /** Solicited issues skipped because they haven't shipped yet
   *  (cover_date is today or later). */
  skipped_solicited: number;
}

/** Fire-and-forget: dispatch a per-issue indexer search for every
 *  missing, already-shipped issue in the catalog. 202 with the summary
 *  counts on success. */
export function searchAllMissing(): Promise<SearchAllMissingSummary> {
  return apiFetch('/pull/search-all-missing', { method: 'POST' });
}
