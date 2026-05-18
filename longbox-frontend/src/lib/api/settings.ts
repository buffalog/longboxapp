import { apiFetch } from './client';

export interface Settings {
  library_root_path: string;
  database_url: string;
  bind_address: string;
  log_level: string;
  match_threshold: number;
  /** Always `true` in Phase A — server boot requires a non-empty key.
   *  Boolean shape preserved for forward-compat if an in-app key flow
   *  ever ships. */
  comicvine_api_key_configured: boolean;
  version: string;
}

export function getSettings(): Promise<Settings> {
  return apiFetch('/settings');
}
