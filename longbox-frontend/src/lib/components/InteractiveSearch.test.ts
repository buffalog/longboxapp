// Component test for InteractiveSearch — the ad-hoc per-issue search
// modal. Covers query pre-population, running a search + rendering the
// scored results table, inline per-indexer errors, and the grab flow
// (submit + close on success).
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api/client';
import { interactiveSearch, grabRelease } from '$lib/api/interactiveSearch';
import { toast } from '$lib/stores/toast.svelte';
import InteractiveSearch from './InteractiveSearch.svelte';

vi.mock('$lib/api/interactiveSearch', () => ({
  interactiveSearch: vi.fn(),
  grabRelease: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

const onClose = vi.fn();

function renderModal() {
  return render(InteractiveSearch, {
    props: {
      open: true,
      onClose,
      seriesId: 42,
      issueId: 7,
      seriesTitle: 'Saga',
      issueNumber: '12'
    }
  });
}

const SAMPLE = {
  results: [
    {
      title: 'Saga 012 (2012) (digital).cbz',
      indexer: 'NZBGeek',
      size_bytes: 45234567,
      age_days: 412,
      confidence: 0.91,
      nzb_url: 'https://idx/nzb/saga'
    },
    {
      title: 'Batman 012 (2016).cbz',
      indexer: 'NZBsu',
      size_bytes: 50000000,
      age_days: 100,
      confidence: 0.2,
      nzb_url: 'https://idx/nzb/batman'
    }
  ],
  errors: [{ indexer: 'DeadIndexer', error: 'timeout' }],
  already_owned: false
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('InteractiveSearch modal', () => {
  it('warns when the library already owns the issue, without hiding results', async () => {
    // The warning is advisory. Interactive Search exists for the case where
    // the engine got it wrong, and "I already have it" is often exactly why
    // someone is here — replacing a bad scan, a wrong language, a PDF they
    // want as a CBZ. So the banner appears AND every result stays grabbable.
    vi.mocked(interactiveSearch).mockResolvedValue({ ...SAMPLE, already_owned: true });
    renderModal();

    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));

    await waitFor(() => {
      expect(screen.getByText(/You already have this issue/i)).toBeInTheDocument();
    });
    expect(screen.getByText('Saga 012 (2012) (digital).cbz')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /Grab/i }).length).toBeGreaterThan(0);
  });

  it('shows no ownership warning when the library does not have the issue', async () => {
    vi.mocked(interactiveSearch).mockResolvedValue(SAMPLE); // already_owned: false
    renderModal();

    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));

    await waitFor(() => {
      expect(screen.getByText('Saga 012 (2012) (digital).cbz')).toBeInTheDocument();
    });
    expect(screen.queryByText(/You already have this issue/i)).not.toBeInTheDocument();
  });

  it('shows no ownership warning before a search has run', async () => {
    vi.mocked(interactiveSearch).mockResolvedValue({ ...SAMPLE, already_owned: true });
    renderModal();
    // Nothing has been searched yet — the flag is unknown, not false.
    expect(screen.queryByText(/you already have this issue/i)).not.toBeInTheDocument();
  });

  it('clears a prior ownership warning when reopened for a different issue', async () => {
    // The modal is mounted once and REUSED (it lives outside any {#if} on
    // both the series and missing pages), so every piece of per-issue state
    // has to be reset by the open-effect. A warning that survives into the
    // next issue's modal is a lie about a different comic.
    vi.mocked(interactiveSearch).mockResolvedValue({ ...SAMPLE, already_owned: true });
    const { rerender } = renderModal();

    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));
    await waitFor(() => {
      expect(screen.getByText(/you already have this issue/i)).toBeInTheDocument();
    });

    // Close, then reopen on another issue — as the parent pages do.
    await rerender({ open: false, onClose, seriesId: 42, issueId: 7, seriesTitle: 'Saga', issueNumber: '12' });
    await rerender({ open: true, onClose, seriesId: 42, issueId: 8, seriesTitle: 'Saga', issueNumber: '13' });

    await waitFor(() => {
      expect(screen.queryByText(/you already have this issue/i)).not.toBeInTheDocument();
    });
  });

  it('pre-populates the query from series title + issue number', () => {
    renderModal();
    const input = screen.getByLabelText('Search query') as HTMLInputElement;
    expect(input.value).toBe('Saga 12');
  });

  it('runs a search and renders scored results plus inline errors', async () => {
    vi.mocked(interactiveSearch).mockResolvedValue(SAMPLE);
    renderModal();

    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));

    await waitFor(() => {
      expect(screen.getByText('Saga 012 (2012) (digital).cbz')).toBeInTheDocument();
    });
    // Both results shown regardless of confidence.
    expect(screen.getByText('Batman 012 (2016).cbz')).toBeInTheDocument();
    // Confidence rendered as a percentage.
    expect(screen.getByText('91%')).toBeInTheDocument();
    // Inline per-indexer error surfaced.
    expect(screen.getByText(/timeout/)).toBeInTheDocument();
    expect(screen.getByText(/DeadIndexer/)).toBeInTheDocument();

    expect(interactiveSearch).toHaveBeenCalledWith({
      series_id: 42,
      issue_id: 7,
      query: 'Saga 12'
    });
  });

  it('grabs a release and closes on success', async () => {
    vi.mocked(interactiveSearch).mockResolvedValue(SAMPLE);
    vi.mocked(grabRelease).mockResolvedValue({ success: true });
    renderModal();

    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));
    await waitFor(() => screen.getByText('Saga 012 (2012) (digital).cbz'));

    const grabButtons = screen.getAllByRole('button', { name: /Grab/i });
    await fireEvent.click(grabButtons[0]!);

    await waitFor(() => {
      expect(grabRelease).toHaveBeenCalledWith({
        nzb_url: 'https://idx/nzb/saga',
        title: 'Saga 012 (2012) (digital).cbz'
      });
      expect(onClose).toHaveBeenCalled();
    });
  });

  it('keeps the modal open when the downloader rejects the grab', async () => {
    vi.mocked(interactiveSearch).mockResolvedValue(SAMPLE);
    vi.mocked(grabRelease).mockResolvedValue({ success: false, error: 'Damaged NZB' });
    renderModal();

    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));
    await waitFor(() => screen.getByText('Saga 012 (2012) (digital).cbz'));
    await fireEvent.click(screen.getAllByRole('button', { name: /Grab/i })[0]!);

    await waitFor(() => expect(grabRelease).toHaveBeenCalled());
    expect(onClose).not.toHaveBeenCalled();
  });

  it('warns when the search request fails', async () => {
    vi.mocked(interactiveSearch).mockRejectedValue(new ApiError(500, 'internal', 'boom'));
    renderModal();
    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));
    await waitFor(() => expect(toast.warning).toHaveBeenCalledWith('boom'));
  });

  it('re-sorts the table when a column header is clicked', async () => {
    vi.mocked(interactiveSearch).mockResolvedValue(SAMPLE);
    renderModal();
    await fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));
    await waitFor(() => screen.getByText('Saga 012 (2012) (digital).cbz'));

    // Default sort is confidence-desc → Saga (0.91) ahead of Batman (0.2).
    const titlesOf = () =>
      screen
        .getAllByRole('row')
        .slice(1)
        .map((r) => r.textContent ?? '');
    expect(titlesOf()[0]).toContain('Saga');

    // Sort by Title ascending → Batman ahead of Saga.
    await fireEvent.click(screen.getByRole('button', { name: /^Title/ }));
    expect(titlesOf()[0]).toContain('Batman');
  });
});
