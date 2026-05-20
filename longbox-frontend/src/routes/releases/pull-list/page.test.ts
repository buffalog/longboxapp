// Component test for the pull-list management page. The route
// +page.svelte is an ordinary component taking a `data` prop, so it
// renders in isolation with @testing-library/svelte.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { checkPull, removeFromPullList, setPullPaused, type PullListEntry } from '$lib/api/pull';
import PullListPage from './+page.svelte';

vi.mock('$lib/api/pull', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/pull')>()),
  checkPull: vi.fn(),
  removeFromPullList: vi.fn(),
  setPullPaused: vi.fn()
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

describe('pull-list page', () => {
  it('lists the subscribed series', () => {
    render(PullListPage, { props: { data: { libraryRoot: null, entries: [sampleListEntry()] } } });
    expect(screen.getByText('Saga')).toBeInTheDocument();
  });

  it('shows an empty state when nothing is subscribed', () => {
    render(PullListPage, { props: { data: { libraryRoot: null, entries: [] } } });
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
    render(PullListPage, { props: { data: { libraryRoot: null, entries: [sampleListEntry()] } } });

    await fireEvent.click(screen.getByRole('button', { name: 'Pause' }));

    await waitFor(() => expect(setPullPaused).toHaveBeenCalledWith(1, true));
    expect(await screen.findByText('Paused')).toBeInTheDocument();
  });

  it('removes a series from the list', async () => {
    vi.mocked(removeFromPullList).mockResolvedValue(undefined);
    render(PullListPage, { props: { data: { libraryRoot: null, entries: [sampleListEntry()] } } });

    await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => expect(removeFromPullList).toHaveBeenCalledWith(1));
    await waitFor(() => expect(screen.queryByText('Saga')).not.toBeInTheDocument());
  });

  it('triggers a sweep with Check now', async () => {
    vi.mocked(checkPull).mockResolvedValue(undefined);
    render(PullListPage, { props: { data: { libraryRoot: null, entries: [] } } });

    await fireEvent.click(screen.getByRole('button', { name: 'Check now' }));
    await waitFor(() => expect(checkPull).toHaveBeenCalledTimes(1));
  });
});
