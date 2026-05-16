import { apiFetch } from './client';
import type { SeriesSearchResult } from '../types';

export function searchVolumes(query: string): Promise<SeriesSearchResult[]> {
  const params = new URLSearchParams({ q: query });
  return apiFetch(`/cv/search?${params.toString()}`);
}
