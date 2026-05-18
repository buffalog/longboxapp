import { apiFetch } from './client';
import type { SeriesWithCounts } from '../types';

export interface DashboardActivity {
  recent_series: SeriesWithCounts[];
  recent_matches: RecentMatch[];
}

export interface RecentMatch {
  file_id: number;
  path_relative: string;
  matched_at: string;
  issue: {
    id: number;
    number: string;
    title: string | null;
    cover_url: string | null;
  };
  series: {
    id: number;
    title: string;
    start_year: number | null;
  };
}

export function getActivity(limit = 6): Promise<DashboardActivity> {
  return apiFetch(`/dashboard/activity?limit=${limit}`);
}
