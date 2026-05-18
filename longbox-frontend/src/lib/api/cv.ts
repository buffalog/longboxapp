import { apiFetch } from './client';
import type { SeriesSearchResult } from '../types';

export interface CvSearchResponse {
  results: SeriesSearchResult[];
  /** How many results the publisher blocklist removed for this call. */
  filtered_count: number;
}

export function searchVolumes(
  query: string,
  options: { showFiltered?: boolean } = {}
): Promise<CvSearchResponse> {
  const params = new URLSearchParams({ q: query });
  if (options.showFiltered) params.set('show_filtered', 'true');
  return apiFetch(`/cv/search?${params.toString()}`);
}
