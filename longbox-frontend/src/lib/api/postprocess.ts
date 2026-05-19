import { apiFetch } from './client';
import type { PendingResponse } from '../types';

/** Read the Phase B pending-intervention cache. Single endpoint used by
 *  both the dashboard counter tile and the /files/pending-intervention
 *  list view — count + items in one round trip so the dashboard's tile
 *  doesn't need a separate "just the count" call. */
export function getPendingInterventions(): Promise<PendingResponse> {
  return apiFetch('/postprocess/pending');
}
