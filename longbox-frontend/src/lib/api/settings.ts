import { apiFetch } from './client';

export interface Settings {
  library_root_path: string;
  database_url: string;
  bind_address: string;
  log_level: string;
  /** Historical env-display field. Reflects the boot-time
   *  `MATCH_THRESHOLD` env var, not the live tunable. The actually
   *  load-bearing value the scanner reads per-scan is
   *  `match_confidence_threshold` below. */
  match_threshold: number;
  /** Always `true` in Phase A — server boot requires a non-empty key.
   *  Boolean shape preserved for forward-compat if an in-app key flow
   *  ever ships. */
  comicvine_api_key_configured: boolean;
  /** Phase B's `DOWNLOAD_WATCH_PATH`. `null` = Phase B not enabled
   *  on this deployment. */
  download_watch_path: string | null;
  version: string;

  /** Runtime-tunable: catalog match threshold. Used by BOTH the
   *  scanner (per scan run) and Phase B (per sweep) — the two
   *  consumers agree on what "confident enough to claim" means by
   *  reading this single row. Falls back to the boot env value when
   *  no DB row exists. */
  match_confidence_threshold: number;
  /** Runtime-tunable: pull engine's NZB-to-series similarity gate.
   *  Separate from `match_confidence_threshold` because pre-grab
   *  errors are asymmetrically costly (wasted SAB cycles + catalog
   *  poison), so the default sits stricter. Read per-sweep by the
   *  pull engine. Default 0.75. */
  pull_indexer_match_threshold: number;
  /** Runtime-tunable: CV enrichment title-similarity gate when the
   *  candidate volume has a known start year. Default 0.85. */
  cv_enrichment_title_threshold_year_known: number;
  /** Runtime-tunable: stricter title-similarity gate when the
   *  candidate has no start year. Default 0.95. */
  cv_enrichment_title_threshold_year_unknown: number;
  /** Runtime-tunable: comma-separated release-title substrings the
   *  pull pre-grab filter drops. Empty string when unset. */
  pull_exclusion_keywords: string;
  /** Runtime-tunable: host-side library path prefix used to render
   *  `file://` URLs for the Show-in-Finder affordance. Empty when
   *  unset (the frontend then shows the container path copy-only). */
  host_library_path: string;
  /** Runtime-tunable: Phase B size floor in megabytes. CBR/CBZ/CB7
   *  files smaller than this get rejected with a WARN log and stay
   *  in /watch/ — catches partial/corrupt SAB deliveries. Default 35
   *  (seeded by migration). `0` disables the floor entirely. */
  min_file_size_mb: number;
}

/** A whitelisted runtime-tunable setting. The backend rejects any
 *  other key with 400 — the env-baked rows (library_root_path, etc.)
 *  are intentionally not exposed to live mutation. */
export type EditableSettingKey =
  | 'match_confidence_threshold'
  | 'pull_indexer_match_threshold'
  | 'cv_enrichment_title_threshold_year_known'
  | 'cv_enrichment_title_threshold_year_unknown'
  | 'pull_exclusion_keywords'
  | 'host_library_path'
  | 'min_file_size_mb';

export function getSettings(): Promise<Settings> {
  return apiFetch('/settings');
}

/** PUT /api/settings/:key. Body shape is `{value: string}` — thresholds
 *  send the stringified number; keywords send the raw CSV. Returns the
 *  canonical (server-normalized) stored value. */
export function updateSetting(
  key: EditableSettingKey,
  value: string
): Promise<{ key: string; value: string }> {
  return apiFetch(`/settings/${key}`, {
    method: 'PUT',
    body: JSON.stringify({ value })
  });
}
