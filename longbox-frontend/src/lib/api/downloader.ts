import { apiFetch } from './client';

export type DownloaderKind = 'sab' | 'nzbget';

/** The single configured Usenet downloader. The secret (SABnzbd
 *  apikey / NZBGet password) is never sent to the client — only
 *  `has_secret` reflects whether one is stored. */
export interface Downloader {
  kind: DownloaderKind;
  base_url: string;
  /** NZBGet Basic-auth username; `null` for SABnzbd. */
  username: string | null;
  has_secret: boolean;
  category: string;
  enabled: boolean;
}

/** Save/test payload. A blank `secret` keeps the stored secret (the
 *  server treats blank as "leave unchanged"). */
export interface DownloaderInput {
  kind: DownloaderKind;
  base_url: string;
  username: string | null;
  secret: string;
  category: string;
  enabled: boolean;
}

export interface ConnectionTestResult {
  ok: boolean;
  message: string;
}

/** Resolves to `null` when no downloader is configured. */
export function getDownloader(): Promise<Downloader | null> {
  return apiFetch('/downloader');
}

export function saveDownloader(input: DownloaderInput): Promise<Downloader> {
  return apiFetch('/downloader', { method: 'PUT', body: JSON.stringify(input) });
}

export function clearDownloader(): Promise<void> {
  return apiFetch('/downloader', { method: 'DELETE' });
}

export function testDownloader(input: DownloaderInput): Promise<ConnectionTestResult> {
  return apiFetch('/downloader/test', { method: 'POST', body: JSON.stringify(input) });
}
