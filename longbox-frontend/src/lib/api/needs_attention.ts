import { apiFetch } from './client';

/** One failed pull surfaced on `/needs-attention` — the latest attempt
 *  for an issue, when that attempt failed. One per issue. */
export interface PullFailure {
  /** `pull_attempts.id` — the dismiss endpoint's key. */
  id: number;
  series_id: number;
  issue_id: number;
  series_title: string;
  issue_number: string;
  /** `null` for a submission failure, set for a grab failure. */
  release_id: string | null;
  error_message: string | null;
  retry_count: number;
  attempted_at: string;
  /** `submission_failed` | `grab_failed`. */
  category: string;
}

/** Issues whose most recent pull attempt failed. */
export function getPullFailures(): Promise<PullFailure[]> {
  return apiFetch('/needs-attention/pull-failures');
}

/** Un-park an issue — clear its failed attempts and nudge an immediate
 *  sweep so the pull engine retries it. */
export function retryPull(seriesId: number, issueId: number): Promise<{ cleared: number }> {
  return apiFetch('/needs-attention/retry', {
    method: 'POST',
    body: JSON.stringify({ series_id: seriesId, issue_id: issueId })
  });
}

/** Dismiss a single pull-failure surface row by `pull_attempts.id`.
 *  Surgical — does not touch other attempts for the same issue. */
export function dismissPullFailure(attemptId: number): Promise<void> {
  return apiFetch(`/needs-attention/pull-failures/${attemptId}`, { method: 'DELETE' });
}

/** Bulk-dismiss every failure-class pull attempt. */
export function clearAllPullFailures(): Promise<void> {
  return apiFetch('/needs-attention/pull-failures', { method: 'DELETE' });
}
