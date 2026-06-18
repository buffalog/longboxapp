// Component test for InteractiveSearch — the ad-hoc per-issue search
// modal. Covers query pre-population, running a search + rendering the
// scored results table, inline per-indexer errors, and the grab flow
// (submit + close on success).
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { interactiveSearch, grabRelease } from '$lib/api/interactiveSearch';
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
  errors: [{ indexer: 'DeadIndexer', error: 'timeout' }]
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('InteractiveSearch modal', () => {
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
    await fireEvent.click(grabButtons[0]);

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
    await fireEvent.click(screen.getAllByRole('button', { name: /Grab/i })[0]);

    await waitFor(() => expect(grabRelease).toHaveBeenCalled());
    expect(onClose).not.toHaveBeenCalled();
  });
});
