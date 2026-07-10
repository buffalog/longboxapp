import { apiFetch } from './client';
import type { SeriesSearchResult } from '../types';

/** A CV search result plus the web layer's local-library annotation. */
export interface CvSearchResultItem extends SeriesSearchResult {
  /**
   * Local series id when this CV volume is already tracked, else null.
   * Drives the "In Library" badge + "View Series" link on the Add page.
   * Optional so existing mocks that predate the field still typecheck.
   */
  in_library_series_id?: number | null;
}

export interface CvSearchResponse {
  results: CvSearchResultItem[];
  /** Results hidden because their publisher is on the blocklist. */
  filtered_publisher: number;
  /** Results hidden because the CV volume is already in the library. */
  filtered_in_library: number;
}

export function searchVolumes(
  query: string,
  options: { showFiltered?: boolean } = {}
): Promise<CvSearchResponse> {
  const params = new URLSearchParams({ q: query });
  if (options.showFiltered) params.set('show_filtered', 'true');
  return apiFetch(`/cv/search?${params.toString()}`);
}
