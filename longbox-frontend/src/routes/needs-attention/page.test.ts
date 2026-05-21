// Component test for the needs-attention page — an ordinary component
// taking a `data` prop, rendered in isolation with @testing-library.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { retryPull, type PullFailure } from '$lib/api/needs_attention';
import type { PendingIntervention } from '$lib/types';
import NeedsAttentionPage from './+page.svelte';

vi.mock('$lib/api/needs_attention', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/needs_attention')>()),
  retryPull: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

function pullFailure(over: Partial<PullFailure> = {}): PullFailure {
  return {
    series_id: 1,
    issue_id: 10,
    series_title: 'Saga',
    issue_number: '5',
    release_id: null,
    error_message: 'submit failed: rejected',
    retry_count: 3,
    attempted_at: '2026-05-20T00:00:00',
    category: 'submission_failed',
    ...over
  };
}

function conflictItem(): PendingIntervention {
  return {
    source_path: '/watch/x.cbz',
    target_path: '/library/x.cbz',
    reason: { kind: 'conflict' },
    size: 1024,
    last_attempt: '2026-05-20T00:00:00'
  };
}

function pageData(
  pullFailures: PullFailure[],
  items: PendingIntervention[] = []
) {
  return { props: { data: { pullFailures, pending: { count: items.length, items } } } };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('needs-attention page', () => {
  it('renders the pull-failure and manual-intervention sections', () => {
    render(NeedsAttentionPage, pageData([pullFailure({ series_title: 'Saga' })], [conflictItem()]));
    expect(screen.getByText('Pull failures')).toBeInTheDocument();
    expect(screen.getByText('Saga')).toBeInTheDocument();
    expect(screen.getByText('Submission failed')).toBeInTheDocument();
    expect(screen.getByText('Manual intervention')).toBeInTheDocument();
    expect(screen.getByText('Conflict')).toBeInTheDocument();
  });

  it('shows the empty state when nothing needs attention', () => {
    render(NeedsAttentionPage, pageData([]));
    expect(screen.getByText('Nothing needs attention')).toBeInTheDocument();
  });

  it('retries a pull failure and drops the row', async () => {
    vi.mocked(retryPull).mockResolvedValue({ cleared: 1 });
    render(NeedsAttentionPage, pageData([pullFailure({ series_id: 1, issue_id: 10 })]));

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(retryPull).toHaveBeenCalledWith(1, 10));
    await waitFor(() => expect(screen.queryByText('Saga')).not.toBeInTheDocument());
  });
});
