import { apiFetch } from './client';

/** OPDS catalog access settings. The password hash never leaves the
 *  server — only `has_password` reflects whether one is stored. The API
 *  token IS returned so it can be copied into a reader app. */
export interface Opds {
  enabled: boolean;
  username: string;
  has_password: boolean;
  api_token: string;
  base_url: string;
  /** The catalog root URL to hand to a reader (a copyable link). */
  catalog_url: string;
}

/** Save payload. Omit a field to leave it unchanged; a blank `password`
 *  keeps the stored one (set-only). */
export interface OpdsInput {
  enabled?: boolean;
  username?: string;
  password?: string;
}

export function getOpds(): Promise<Opds> {
  return apiFetch('/opds/settings');
}

export function saveOpds(input: OpdsInput): Promise<Opds> {
  return apiFetch('/opds/settings', { method: 'PUT', body: JSON.stringify(input) });
}

export function regenerateOpdsToken(): Promise<Opds> {
  return apiFetch('/opds/settings/regenerate-token', { method: 'POST' });
}
