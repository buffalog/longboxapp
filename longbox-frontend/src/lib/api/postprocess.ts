import { apiFetch } from './client';
import type { PendingResponse, SweepSummary } from '../types';

/** Read the Phase B pending-intervention cache. Single endpoint used by
 *  both the dashboard counter tile and the /files/pending-intervention
 *  list view — count + items in one round trip so the dashboard's tile
 *  doesn't need a separate "just the count" call. */
export function getPendingInterventions(): Promise<PendingResponse> {
  return apiFetch('/postprocess/pending');
}

/** Fire an on-demand Phase B sweep over the configured watch folder.
 *  Resolves with the per-outcome tally. The backend rejects with 400
 *  when DOWNLOAD_WATCH_PATH is unset and 503 when it's set but
 *  unreadable — both surface as ApiError with a readable message. */
export function triggerPostprocess(): Promise<SweepSummary> {
  return apiFetch('/postprocess/trigger', { method: 'POST' });
}
