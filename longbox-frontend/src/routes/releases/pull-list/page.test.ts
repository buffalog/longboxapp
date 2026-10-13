// Component test for the pull-list management page. The route
// +page.svelte is an ordinary component taking a `data` prop, so it
// renders in isolation with @testing-library/svelte.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  checkPull,
  getRefreshStatus,
  refreshAll,
  removeFromPullList,
  searchSeriesNow,
  setPullPaused,
  type PullListEntry,
  type RefreshStatus
} from '$lib/api/pull';
import { ApiError } from '$lib/api/client';
import PullListPage from './+page.svelte';
import type { PageData } from './$types';

vi.mock('$lib/api/pull', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/pull')>()),
  checkPull: vi.fn(),
  refreshAll: vi.fn(),
  getRefreshStatus: vi.fn(),
  removeFromPullList: vi.fn(),
  searchSeriesNow: vi.fn(),
  setPullPaused: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

function sampleListEntry(over: Partial<PullListEntry> = {}): PullListEntry {
  return {
    series_id: 1,
    series_title: 'Saga',
    series_sort_title: 'saga',
    series_start_year: 2012,
    paused: false,
    added_at: '2026-05-20T00:00:00',
    last_pull_attempt_at: null,
    last_successful_pull_at: null,
    failure_count: 0,
    ...over
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

const IDLE: RefreshStatus = {
  running: false,
  done: null,
  total: null,
  last_full_refresh_at: null
};

function pageData(entries: PullListEntry[], refreshStatus: RefreshStatus = IDLE) {
  return {
    props: {
      data: { libraryRoot: null, entries, pullFailureCount: 0, refreshStatus } satisfies PageData
    }
  };
}

describe('pull-list page', () => {
  it('lists the subscribed series', () => {
    render(PullListPage, pageData([sampleListEntry()]));
    expect(screen.getByText('Saga')).toBeInTheDocument();
  });

  it('shows an empty state when nothing is subscribed', () => {
    render(PullListPage, pageData([]));
    expect(screen.getByText(/No series on the pull list/)).toBeInTheDocument();
  });

  it('pauses a series', async () => {
    vi.mocked(setPullPaused).mockResolvedValue({
      id: 1,
      series_id: 1,
      added_at: '2026-05-20T00:00:00',
      start_issue: null,
      paused: true,
      last_pull_attempt_at: null,
      last_successful_pull_at: null,
      failure_count: 0
    });
    render(PullListPage, pageData([sampleListEntry()]));

    await fireEvent.click(screen.getByRole('button', { name: 'Pause' }));

    await waitFor(() => expect(setPullPaused).toHaveBeenCalledWith(1, true));
    expect(await screen.findByText('Paused')).toBeInTheDocument();
  });

  it('removes a series from the list', async () => {
    vi.mocked(removeFromPullList).mockResolvedValue(undefined);
    render(PullListPage, pageData([sampleListEntry()]));

    await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => expect(removeFromPullList).toHaveBeenCalledWith(1));
    await waitFor(() => expect(screen.queryByText('Saga')).not.toBeInTheDocument());
  });

  it('triggers a sweep with Check now', async () => {
    vi.mocked(checkPull).mockResolvedValue(undefined);
    render(PullListPage, pageData([]));

    await fireEvent.click(screen.getByRole('button', { name: 'Check now' }));
    await waitFor(() => expect(checkPull).toHaveBeenCalledTimes(1));
  });

  it('fires Search now for exactly that row', async () => {
    vi.mocked(searchSeriesNow).mockResolvedValue({ queued: 1, note: null });
    render(
      PullListPage,
      pageData([
        sampleListEntry({ series_id: 1, series_title: 'Saga' }),
        sampleListEntry({ series_id: 2, series_title: 'Chew' })
      ])
    );

    // Two rows, two buttons — click the first one only.
    const buttons = screen.getAllByRole('button', { name: 'Search now' });
    expect(buttons).toHaveLength(2);
    await fireEvent.click(buttons[0]!);

    await waitFor(() => expect(searchSeriesNow).toHaveBeenCalledTimes(1));
    expect(searchSeriesNow).toHaveBeenCalledWith(1);
    // Clicked row's button stays disabled (debounce timer running);
    // the sibling row's button is still clickable.
    expect(buttons[0]).toBeDisabled();
    expect(buttons[1]).not.toBeDisabled();
  });

  it('shows a warning toast when Search now hits 409', async () => {
    const { toast } = await import('$lib/stores/toast.svelte');
    vi.mocked(searchSeriesNow).mockRejectedValue(
      new ApiError(409, 'conflict.pull_search_running', 'A search is already running.')
    );
    render(PullListPage, pageData([sampleListEntry({ series_id: 1, series_title: 'Saga' })]));

    await fireEvent.click(screen.getByRole('button', { name: 'Search now' }));
    await waitFor(() =>
      expect(toast.warning).toHaveBeenCalledWith(
        expect.stringContaining('A search is already running for Saga')
      )
    );
  });

  describe('full catalog refresh', () => {
    it('shows progress while running and disables the button', () => {
      render(
        PullListPage,
        pageData([], { running: true, done: 3, total: 10, last_full_refresh_at: null })
      );
      expect(screen.getByTestId('refresh-status')).toHaveTextContent(
        'Full refresh in progress: 3 of 10 series'
      );
      expect(screen.getByRole('button', { name: 'Refresh all' })).toBeDisabled();
    });

    it('says starting before the target count lands', () => {
      render(PullListPage, pageData([], { running: true, done: 0, total: 0, last_full_refresh_at: null }));
      expect(screen.getByTestId('refresh-status')).toHaveTextContent('Full refresh starting');
    });

    it('shows never, then a relative time, when idle', () => {
      const { unmount } = render(PullListPage, pageData([]));
      expect(screen.getByTestId('refresh-status')).toHaveTextContent('Last full refresh: never');
      unmount();
      const twoHoursAgo = Math.floor(Date.now() / 1000) - 2 * 3600;
      render(PullListPage, pageData([], { ...IDLE, last_full_refresh_at: twoHoursAgo }));
      expect(screen.getByTestId('refresh-status')).toHaveTextContent('Last full refresh: 2h ago');
    });

    it('starts a refresh and reads the new status', async () => {
      vi.mocked(refreshAll).mockResolvedValue(undefined);
      vi.mocked(getRefreshStatus).mockResolvedValue({
        running: true,
        done: 0,
        total: 5,
        last_full_refresh_at: null
      });
      render(PullListPage, pageData([]));
      await fireEvent.click(screen.getByRole('button', { name: 'Refresh all' }));
      expect(refreshAll).toHaveBeenCalledOnce();
      await waitFor(() =>
        expect(screen.getByTestId('refresh-status')).toHaveTextContent('0 of 5 series')
      );
    });

    it('warns on 409 instead of erroring', async () => {
      const { toast } = await import('$lib/stores/toast.svelte');
      vi.mocked(refreshAll).mockRejectedValue(
        new ApiError(409, 'conflict.refresh_running', 'running')
      );
      render(PullListPage, pageData([]));
      await fireEvent.click(screen.getByRole('button', { name: 'Refresh all' }));
      await waitFor(() =>
        expect(toast.warning).toHaveBeenCalledWith('A full catalog refresh is already running.')
      );
    });
  });
});
