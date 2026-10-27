import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getSeriesAliases, setSeriesAliases } from '$lib/api/series';
import SeriesAliases from './SeriesAliases.svelte';

vi.mock('$lib/api/series', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/series')>()),
  getSeriesAliases: vi.fn(),
  setSeriesAliases: vi.fn()
}));

beforeEach(() => {
  vi.clearAllMocks();
});

describe('SeriesAliases', () => {
  it('shows ComicVine aliases read-only and the user aliases for editing', async () => {
    vi.mocked(getSeriesAliases).mockResolvedValue({
      comicvine: ['Collider'],
      user: ['Federal Bureau of Physics']
    });
    render(SeriesAliases, { props: { seriesId: 7 } });

    expect(await screen.findByText('From ComicVine: Collider')).toBeInTheDocument();
    expect(screen.getByLabelText('Aliases')).toHaveValue('Federal Bureau of Physics');
  });

  it('saves one alias per non-blank line', async () => {
    vi.mocked(getSeriesAliases).mockResolvedValue({ comicvine: [], user: [] });
    vi.mocked(setSeriesAliases).mockResolvedValue({
      comicvine: [],
      user: ['Teenage Mutant Ninja Turtles: The Last Ronin', 'TMNT Last Ronin']
    });
    render(SeriesAliases, { props: { seriesId: 7 } });

    const box = await screen.findByLabelText('Aliases');
    await fireEvent.input(box, {
      target: { value: ' Teenage Mutant Ninja Turtles: The Last Ronin \n\nTMNT Last Ronin' }
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Save aliases' }));

    await waitFor(() =>
      expect(setSeriesAliases).toHaveBeenCalledWith(7, [
        'Teenage Mutant Ninja Turtles: The Last Ronin',
        'TMNT Last Ronin'
      ])
    );
  });
});
