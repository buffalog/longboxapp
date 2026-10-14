// Add page: a ComicVine-busy 503 is retried once after its
// retry_after_seconds with a visible "busy" notice, and a slow older
// search can't overwrite a newer one.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { searchVolumes, type CvSearchResponse } from '$lib/api/cv';
import { addSeries, type AddSeriesResponse } from '$lib/api/series';
import { ApiError } from '$lib/api/client';
import AddPage from './+page.svelte';

vi.mock('$lib/api/cv', () => ({ searchVolumes: vi.fn() }));
vi.mock('$lib/api/series', () => ({ addSeries: vi.fn() }));

function response(name: string, cv_id = 1): CvSearchResponse {
  return {
    results: [
      {
        cv_id,
        name,
        start_year: 2020,
        publisher: null,
        issue_count: 3,
        cover_url: null,
        description: null
      }
    ],
    filtered_publisher: 0,
    filtered_in_library: 0
  };
}

const busy = (): ApiError =>
  new ApiError(503, 'rate_limited', 'Rate limited; retry after 1s', { retry_after_seconds: 1 });

async function type(text: string): Promise<void> {
  await fireEvent.input(screen.getByPlaceholderText('Search ComicVine…'), {
    target: { value: text }
  });
}

beforeEach(() => {
  vi.mocked(searchVolumes).mockReset();
  vi.mocked(addSeries).mockReset();
});

describe('Add page', () => {
  it('retries a busy search once, showing the busy notice while it waits', async () => {
    vi.mocked(searchVolumes)
      .mockRejectedValueOnce(busy())
      .mockResolvedValueOnce(response('Saga'));
    render(AddPage);
    await type('saga');

    expect(await screen.findByTestId('cv-busy')).toHaveTextContent(
      'ComicVine is busy, retrying in 1s'
    );
    expect(await screen.findByText('Saga', {}, { timeout: 3000 })).toBeInTheDocument();
    expect(searchVolumes).toHaveBeenCalledTimes(2);
    expect(screen.queryByTestId('cv-busy')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('surfaces the error when the retry is busy too', async () => {
    vi.mocked(searchVolumes).mockRejectedValue(busy());
    render(AddPage);
    await type('saga');

    await waitFor(() => expect(searchVolumes).toHaveBeenCalledTimes(2), { timeout: 3000 });
    expect(await screen.findByText(/Rate limited/)).toBeInTheDocument();
  });

  it('retries a busy add once', async () => {
    vi.mocked(searchVolumes).mockResolvedValue(response('Saga', 7));
    vi.mocked(addSeries)
      .mockRejectedValueOnce(busy())
      .mockResolvedValueOnce({ pull_search_queued: 0 } as AddSeriesResponse);
    render(AddPage);
    await type('saga');
    await fireEvent.click(await screen.findByText('Add to Library'));

    expect(await screen.findByText(/Added "Saga"/, {}, { timeout: 3000 })).toBeInTheDocument();
    expect(addSeries).toHaveBeenCalledTimes(2);
  });

  it('drops a stale search response that lands after a newer one', async () => {
    let resolveOld: (r: CvSearchResponse) => void = () => {};
    vi.mocked(searchVolumes)
      .mockImplementationOnce(() => new Promise((r) => (resolveOld = r)))
      .mockResolvedValueOnce(response('New Result', 2));
    render(AddPage);

    await type('old');
    await waitFor(() => expect(searchVolumes).toHaveBeenCalledTimes(1));
    await type('new');
    expect(await screen.findByText('New Result')).toBeInTheDocument();

    resolveOld(response('Old Result', 1));
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByText('Old Result')).toBeNull();
    expect(screen.getByText('New Result')).toBeInTheDocument();
  });
});
