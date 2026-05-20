// TEMPLATE: this is the first component test in the codebase and the
// pattern the DownloaderSettings / WebhookSettings tests follow.
//
// Shape:
//   1. vi.mock the component's api module — `importOriginal` keeps any
//      non-function exports (constants) real, only CRUD calls become
//      spies. The component is then exercised with no network and no
//      SvelteKit load.
//   2. A `sample*()` factory builds fixtures with per-test overrides.
//   3. Each test renders with props, drives the DOM with `fireEvent`,
//      and asserts both the spy call and the resulting DOM.
//   4. Async handlers settle on a later microtask — assert with
//      `waitFor` / `findBy*`, never a bare `getBy*` straight after a
//      click.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api/client';
import { createIndexer, deleteIndexer, testIndexer, updateIndexer, type Indexer } from '$lib/api/indexers';
import IndexerSettings from './IndexerSettings.svelte';

vi.mock('$lib/api/indexers', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/indexers')>()),
  createIndexer: vi.fn(),
  updateIndexer: vi.fn(),
  deleteIndexer: vi.fn(),
  testIndexer: vi.fn()
}));

function sampleIndexer(over: Partial<Indexer> = {}): Indexer {
  return {
    id: 1,
    name: 'NZBgeek',
    base_url: 'https://api.nzbgeek.info',
    has_api_key: true,
    enabled: true,
    priority: 0,
    maxage_days: 1500,
    ...over
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('IndexerSettings', () => {
  it('lists the indexers it was given', () => {
    render(IndexerSettings, { props: { indexers: [sampleIndexer()] } });
    expect(screen.getByText('NZBgeek')).toBeInTheDocument();
    expect(screen.queryByText('No indexers configured.')).not.toBeInTheDocument();
  });

  it('shows an empty state when there are no indexers', () => {
    render(IndexerSettings, { props: { indexers: [] } });
    expect(screen.getByText('No indexers configured.')).toBeInTheDocument();
  });

  it('creates an indexer and appends it to the list', async () => {
    vi.mocked(createIndexer).mockResolvedValue(sampleIndexer({ id: 2, name: 'DrunkenSlug' }));
    render(IndexerSettings, { props: { indexers: [] } });

    await fireEvent.click(screen.getByRole('button', { name: '+ Add indexer' }));
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'DrunkenSlug' } });
    await fireEvent.input(screen.getByLabelText('Base URL'), {
      target: { value: 'https://api.drunkenslug.com' }
    });
    await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'KEY' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Add indexer' }));

    await waitFor(() =>
      expect(createIndexer).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'DrunkenSlug',
          base_url: 'https://api.drunkenslug.com',
          api_key: 'KEY'
        })
      )
    );
    expect(await screen.findByText('DrunkenSlug')).toBeInTheDocument();
  });

  it('edits an indexer, sending a blank api_key to keep the stored key', async () => {
    vi.mocked(updateIndexer).mockResolvedValue(sampleIndexer({ name: 'Renamed' }));
    render(IndexerSettings, { props: { indexers: [sampleIndexer()] } });

    await fireEvent.click(screen.getByRole('button', { name: 'Edit indexer NZBgeek' }));
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Renamed' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(updateIndexer).toHaveBeenCalledTimes(1));
    const [id, input] = vi.mocked(updateIndexer).mock.calls[0]!;
    expect(id).toBe(1);
    expect(input.name).toBe('Renamed');
    // Blank api_key is the "keep the stored key" signal.
    expect(input.api_key).toBe('');
    expect(await screen.findByText('Renamed')).toBeInTheDocument();
  });

  it('deletes an indexer and drops it from the list', async () => {
    vi.mocked(deleteIndexer).mockResolvedValue(undefined);
    render(IndexerSettings, { props: { indexers: [sampleIndexer()] } });

    await fireEvent.click(screen.getByRole('button', { name: 'Remove indexer NZBgeek' }));

    await waitFor(() => expect(deleteIndexer).toHaveBeenCalledWith(1));
    await waitFor(() => expect(screen.queryByText('NZBgeek')).not.toBeInTheDocument());
  });

  it('runs a connection test and shows the result', async () => {
    vi.mocked(testIndexer).mockResolvedValue({
      ok: true,
      message: 'Indexer reachable and API key accepted.'
    });
    render(IndexerSettings, { props: { indexers: [sampleIndexer()] } });

    await fireEvent.click(screen.getByRole('button', { name: 'Test' }));

    expect(await screen.findByText(/reachable and API key accepted/)).toBeInTheDocument();
    // An existing row tests with a blank key — the server re-uses the stored one.
    expect(testIndexer).toHaveBeenCalledWith(expect.objectContaining({ id: 1, api_key: '' }));
  });

  it('surfaces an ApiError when a mutation fails', async () => {
    vi.mocked(createIndexer).mockRejectedValue(
      new ApiError(409, 'conflict.indexer_exists', 'An indexer named "X" already exists.')
    );
    render(IndexerSettings, { props: { indexers: [] } });

    await fireEvent.click(screen.getByRole('button', { name: '+ Add indexer' }));
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'X' } });
    await fireEvent.input(screen.getByLabelText('Base URL'), { target: { value: 'https://x' } });
    await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'K' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Add indexer' }));

    expect(await screen.findByText(/already exists/)).toBeInTheDocument();
  });
});
