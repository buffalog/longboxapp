import { apiFetch } from './client';

export interface CreatorSearchRow {
  id: number; name: string; cv_person_id: number | null;
  series_count: number; issue_count: number;
}
export interface RoleCount { role: string; count: number }
export interface CreatorSeries { series_id: number; name: string; issue_count: number; cover_url: string | null }
export interface CreatorDetail {
  id: number; name: string; cv_person_id: number | null;
  roles: RoleCount[]; series: CreatorSeries[];
}
export interface CreatorIssueRow {
  issue_id: number; series_name: string; issue_number: string;
  cover_date: string | null; cover_url: string | null; role: string;
}

export function searchCreators(q: string): Promise<CreatorSearchRow[]> {
  return apiFetch(`/creators/search?q=${encodeURIComponent(q)}`);
}
export function getCreator(id: number): Promise<CreatorDetail> {
  return apiFetch(`/creators/${id}`);
}
export function getCreatorIssues(
  id: number, opts: { role?: string; series_id?: number; page?: number } = {},
): Promise<CreatorIssueRow[]> {
  const p = new URLSearchParams();
  if (opts.role) p.set('role', opts.role);
  if (opts.series_id != null) p.set('series_id', String(opts.series_id));
  if (opts.page != null) p.set('page', String(opts.page));
  const qs = p.toString();
  return apiFetch(`/creators/${id}/issues${qs ? `?${qs}` : ''}`);
}

export interface DiscoveredVolume {
  cv_volume_id: number;
  name: string;
  series_id: number | null; // non-null => already in the library
}

export function getCreatorDiscovery(id: number): Promise<DiscoveredVolume[]> {
  return apiFetch(`/creators/${id}/discover`);
}
