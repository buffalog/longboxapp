// Mirrors the IndexerSettings.test.ts template.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api/client';
import { addToPullList, removeFromPullList, setPullPaused, type PullEntry } from '$lib/api/pull';
import PullListToggle from './PullListToggle.svelte';

vi.mock('$lib/api/pull', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/pull')>()),
  addToPullList: vi.fn(),
  removeFromPullList: vi.fn(),
  setPullPaused: vi.fn()
}));

function sampleEntry(over: Partial<PullEntry> = {}): PullEntry {
  return {
    id: 1,
    series_id: 42,
    added_at: '2026-05-20T00:00:00',
    start_issue: null,
    paused: false,
    last_pull_attempt_at: null,
    last_successful_pull_at: null,
    failure_count: 0,
    ...over
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('PullListToggle', () => {
  it('offers to subscribe when the series is not on the list', () => {
    render(PullListToggle, { props: { seriesId: 42, entry: null } });
    expect(screen.getByRole('button', { name: '+ Pull list' })).toBeInTheDocument();
  });

  it('subscribes the series', async () => {
    vi.mocked(addToPullList).mockResolvedValue(sampleEntry());
    render(PullListToggle, { props: { seriesId: 42, entry: null } });

    await fireEvent.click(screen.getByRole('button', { name: '+ Pull list' }));

    await waitFor(() => expect(addToPullList).toHaveBeenCalledWith(42));
    expect(await screen.findByText('On pull list')).toBeInTheDocument();
  });

  it('shows the subscribed state and pauses', async () => {
    vi.mocked(setPullPaused).mockResolvedValue(sampleEntry({ paused: true }));
    render(PullListToggle, { props: { seriesId: 42, entry: sampleEntry() } });

    expect(screen.getByText('On pull list')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Pause' }));

    await waitFor(() => expect(setPullPaused).toHaveBeenCalledWith(42, true));
    expect(await screen.findByText('Pulls paused')).toBeInTheDocument();
  });

  it('removes the series from the list', async () => {
    vi.mocked(removeFromPullList).mockResolvedValue(undefined);
    render(PullListToggle, { props: { seriesId: 42, entry: sampleEntry() } });

    await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => expect(removeFromPullList).toHaveBeenCalledWith(42));
    expect(await screen.findByRole('button', { name: '+ Pull list' })).toBeInTheDocument();
  });

  it('surfaces an ApiError', async () => {
    vi.mocked(addToPullList).mockRejectedValue(
      new ApiError(
        409,
        'conflict.already_on_pull_list',
        'That series is already on the pull list.'
      )
    );
    render(PullListToggle, { props: { seriesId: 42, entry: null } });

    await fireEvent.click(screen.getByRole('button', { name: '+ Pull list' }));
    expect(await screen.findByText(/already on the pull list/)).toBeInTheDocument();
  });
});
