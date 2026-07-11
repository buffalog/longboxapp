import { apiFetch } from './client';

export interface PublisherFilter {
  id: number;
  publisher_name: string;
  /** `block` | `allow`. Phase A UI only writes `block`. */
  mode: 'block' | 'allow';
  created_at: string;
}

export function listFilters(): Promise<PublisherFilter[]> {
  return apiFetch('/publishers/filters');
}

export function addFilter(publisherName: string): Promise<PublisherFilter> {
  return apiFetch('/publishers/filters', {
    method: 'POST',
    body: JSON.stringify({ publisher_name: publisherName })
  });
}

export function deleteFilter(id: number): Promise<void> {
  return apiFetch(`/publishers/filters/${id}`, { method: 'DELETE' });
}

export function resetFiltersToDefaults(): Promise<{ inserted: number }> {
  return apiFetch('/publishers/filters/reset-defaults', { method: 'POST' });
}

/** Recommended reprint publishers the user hasn't blocked yet. */
export function listRecommendedFilters(): Promise<string[]> {
  return apiFetch('/publishers/filters/recommended');
}

/** Bulk-add selected recommended publishers; returns the rows inserted. */
export function addRecommendedFilters(
  publisherNames: string[]
): Promise<{ added: PublisherFilter[] }> {
  return apiFetch('/publishers/filters/recommended', {
    method: 'POST',
    body: JSON.stringify({ publisher_names: publisherNames })
  });
}
