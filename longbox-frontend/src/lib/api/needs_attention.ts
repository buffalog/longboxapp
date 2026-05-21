import { apiFetch } from './client';

/** One failed pull surfaced on `/needs-attention` — the latest attempt
 *  for an issue, when that attempt failed. One per issue. */
export interface PullFailure {
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
