import { apiFetch } from './client';

/** A Newznab indexer row. The API key is never sent to the client —
 *  only `has_api_key` reflects whether one is stored. */
export interface Indexer {
  id: number;
  name: string;
  base_url: string;
  has_api_key: boolean;
  enabled: boolean;
  priority: number;
  maxage_days: number;
}

/** Create/update payload. On update, a blank `api_key` keeps the
 *  stored key (the server treats blank as "leave unchanged"). */
export interface IndexerInput {
  name: string;
  base_url: string;
  api_key: string;
  enabled: boolean;
  priority: number;
  maxage_days: number;
}

export interface ConnectionTestResult {
  ok: boolean;
  message: string;
}

export function listIndexers(): Promise<Indexer[]> {
  return apiFetch('/indexers');
}

export function createIndexer(input: IndexerInput): Promise<Indexer> {
  return apiFetch('/indexers', { method: 'POST', body: JSON.stringify(input) });
}

export function updateIndexer(id: number, input: IndexerInput): Promise<Indexer> {
  return apiFetch(`/indexers/${id}`, { method: 'PUT', body: JSON.stringify(input) });
}

export function deleteIndexer(id: number): Promise<void> {
  return apiFetch(`/indexers/${id}`, { method: 'DELETE' });
}

/** Probe an indexer. Passing `id` with a blank `api_key` re-uses the
 *  stored key, so an existing indexer can be re-tested without
 *  re-typing it. */
export function testIndexer(input: {
  id?: number;
  name?: string;
  base_url: string;
  api_key: string;
  maxage_days?: number;
}): Promise<ConnectionTestResult> {
  return apiFetch('/indexers/test', { method: 'POST', body: JSON.stringify(input) });
}
