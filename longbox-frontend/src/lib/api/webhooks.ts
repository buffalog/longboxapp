import { apiFetch } from './client';

// Webhook event bits — a mirror of the EVENT_* consts in
// longbox-db/src/webhook_config_repo.rs. Stable and append-only: a new
// event takes the next free bit, never reuses one.
//
// NOTE (Phase A.8 Step 5 kickoff, flag i): this is a hand-maintained
// copy of a server-side list — acceptable at four events. If the event
// set grows, replace this with server-provided event metadata (e.g. a
// GET /webhooks/events endpoint) rather than keeping two lists in sync
// by hand.
export const WEBHOOK_EVENTS = [
  { bit: 1, label: 'Pull succeeded' },
  { bit: 2, label: 'Pull failed' },
  { bit: 4, label: 'New solicitations' },
  { bit: 8, label: 'Pull engine error' }
] as const;

export interface Webhook {
  id: number;
  name: string;
  url: string;
  /** Bitset of WEBHOOK_EVENTS bits this webhook fires for. */
  event_mask: number;
  enabled: boolean;
  created_at: string;
}

export interface WebhookInput {
  name: string;
  url: string;
  event_mask: number;
  enabled: boolean;
}

export function listWebhooks(): Promise<Webhook[]> {
  return apiFetch('/webhooks');
}

export function createWebhook(input: WebhookInput): Promise<Webhook> {
  return apiFetch('/webhooks', { method: 'POST', body: JSON.stringify(input) });
}

export function updateWebhook(id: number, input: WebhookInput): Promise<Webhook> {
  return apiFetch(`/webhooks/${id}`, { method: 'PUT', body: JSON.stringify(input) });
}

export function deleteWebhook(id: number): Promise<void> {
  return apiFetch(`/webhooks/${id}`, { method: 'DELETE' });
}

/** Deliver a synthetic test notification to a webhook's URL. Rejects
 *  with `ApiError` (422) when the endpoint is unreachable or errors. */
export function testWebhook(id: number): Promise<{ delivered: boolean }> {
  return apiFetch(`/webhooks/${id}/test`, { method: 'POST' });
}
