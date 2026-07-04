import { apiFetch } from './client';

export interface PageCount {
  count: number;
}

export interface ReadingProgress {
  last_page: number;
}

/** Minimal issue shape the reader needs — `series_id` drives exit navigation. */
export interface ReaderIssue {
  id: number;
  series_id: number;
  number: string;
  title: string | null;
}

export function getPageCount(issueId: number): Promise<PageCount> {
  return apiFetch(`/issues/${issueId}/pages/count`);
}

export function getIssue(issueId: number): Promise<ReaderIssue> {
  return apiFetch(`/issues/${issueId}`);
}

export function getReadingProgress(issueId: number): Promise<ReadingProgress> {
  return apiFetch(`/issues/${issueId}/reading-progress`);
}

export function saveReadingProgress(issueId: number, lastPage: number): Promise<{ ok: boolean }> {
  return apiFetch(`/issues/${issueId}/reading-progress`, {
    method: 'PUT',
    body: JSON.stringify({ last_page: lastPage })
  });
}

/** Direct URL for a page image — used as an `<img src>` / preload target,
 *  not fetched through `apiFetch` (the response is image bytes, not JSON). */
export function pageImageUrl(issueId: number, page: number): string {
  return `/api/issues/${issueId}/pages/${page}`;
}
