import { apiFetch } from './client';

/** Global OPDS settings. Per-user credentials live in {@link OpdsUser};
 *  this is just the master toggle plus the info needed to render a copyable
 *  catalog URL. */
export interface Opds {
  enabled: boolean;
  /** Dedicated OPDS listener port (default 8096). */
  opds_port: number;
  /** Configured public base URL (`OPDS_BASE_URL`), or empty. When set it's
   *  the authoritative catalog origin; otherwise the UI composes the URL
   *  from the browser's host + `opds_port`. */
  base_url: string;
}

/** One OPDS account. The password hash never leaves the server. */
export interface OpdsUser {
  id: number;
  username: string;
  enabled: boolean;
  /** "YYYY-MM-DD HH:MM:SS" UTC. */
  created_at: string;
  /** Last successful auth, or null if never used. */
  last_seen_at: string | null;
}

export interface OpdsSettingsInput {
  enabled?: boolean;
}

export interface CreateOpdsUserInput {
  username: string;
  password: string;
}

export function getOpds(): Promise<Opds> {
  return apiFetch('/opds/settings');
}

export function saveOpds(input: OpdsSettingsInput): Promise<Opds> {
  return apiFetch('/opds/settings', { method: 'PUT', body: JSON.stringify(input) });
}

export function listOpdsUsers(): Promise<OpdsUser[]> {
  return apiFetch('/opds/users');
}

export function createOpdsUser(input: CreateOpdsUserInput): Promise<OpdsUser> {
  return apiFetch('/opds/users', { method: 'POST', body: JSON.stringify(input) });
}

export function enableOpdsUser(id: number): Promise<{ id: number; enabled: boolean }> {
  return apiFetch(`/opds/users/${id}/enable`, { method: 'POST' });
}

export function disableOpdsUser(id: number): Promise<{ id: number; enabled: boolean }> {
  return apiFetch(`/opds/users/${id}/disable`, { method: 'POST' });
}

export function deleteOpdsUser(id: number): Promise<{ deleted: number }> {
  return apiFetch(`/opds/users/${id}`, { method: 'DELETE' });
}
