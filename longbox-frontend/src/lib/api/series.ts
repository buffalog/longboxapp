import { apiFetch } from './client';
import type { Series, SeriesDetail, SeriesWithCounts } from '../types';

export function listSeries(): Promise<SeriesWithCounts[]> {
  return apiFetch('/series');
}

export function getSeries(id: number): Promise<SeriesDetail> {
  return apiFetch(`/series/${id}`);
}

export function addSeries(cvId: number): Promise<Series> {
  return apiFetch('/series', {
    method: 'POST',
    body: JSON.stringify({ cv_id: cvId })
  });
}

export function deleteSeries(id: number): Promise<{ deleted: number }> {
  return apiFetch(`/series/${id}`, { method: 'DELETE' });
}

export function refreshSeries(id: number): Promise<Series> {
  return apiFetch(`/series/${id}/refresh`, { method: 'POST' });
}
