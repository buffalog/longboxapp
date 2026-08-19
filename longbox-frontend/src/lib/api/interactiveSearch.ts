import { apiFetch } from './client';

/** One NZB release from an interactive search. `nzb_url` is round-tripped
 *  back to the grab endpoint as-is. */
export interface InteractiveSearchResult {
  title: string;
  indexer: string;
  size_bytes: number | null;
  age_days: number | null;
  /** Series-title match score, 0.0–1.0. */
  confidence: number;
  nzb_url: string;
}

/** One indexer that failed during the search — shown inline so partial
 *  results from the other indexers still display. */
export interface IndexerSearchError {
  indexer: string;
  error: string;
}

export interface InteractiveSearchResponse {
  results: InteractiveSearchResult[];
  errors: IndexerSearchError[];
  /** The library already holds an owned, present file for this issue.
   *  A warning only — results are still returned and grab still works. */
  already_owned: boolean;
}

export interface GrabResponse {
  success: boolean;
  error?: string;
}

/** Run an ad-hoc search for a specific issue across all configured indexers. */
export function interactiveSearch(input: {
  series_id: number;
  issue_id: number;
  query: string;
}): Promise<InteractiveSearchResponse> {
  return apiFetch('/search/interactive', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}

/** Submit a chosen release to the configured downloader. */
export function grabRelease(input: { nzb_url: string; title: string }): Promise<GrabResponse> {
  return apiFetch('/search/interactive/grab', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}
