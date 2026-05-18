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
