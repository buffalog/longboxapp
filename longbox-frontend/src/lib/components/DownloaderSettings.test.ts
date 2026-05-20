// Mirrors the IndexerSettings.test.ts template — see that file for the
// rationale behind the mock / fixture / fireEvent + waitFor shape.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api/client';
import { clearDownloader, saveDownloader, testDownloader, type Downloader } from '$lib/api/downloader';
import DownloaderSettings from './DownloaderSettings.svelte';

vi.mock('$lib/api/downloader', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/downloader')>()),
  saveDownloader: vi.fn(),
  clearDownloader: vi.fn(),
  testDownloader: vi.fn()
}));

function sampleDownloader(over: Partial<Downloader> = {}): Downloader {
  return {
    kind: 'sab',
    base_url: 'http://localhost:8080',
    username: null,
    has_secret: true,
    category: 'comics',
    enabled: true,
    ...over
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('DownloaderSettings', () => {
  it('renders the saved downloader configuration', () => {
    render(DownloaderSettings, { props: { downloader: sampleDownloader() } });
    expect((screen.getByLabelText('Base URL') as HTMLInputElement).value).toBe(
      'http://localhost:8080'
    );
  });

  it('saves the downloader configuration', async () => {
    vi.mocked(saveDownloader).mockResolvedValue(sampleDownloader());
    render(DownloaderSettings, { props: { downloader: null } });

    await fireEvent.input(screen.getByLabelText('Base URL'), {
      target: { value: 'http://sab:8080' }
    });
    await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'APIKEY' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(saveDownloader).toHaveBeenCalledWith(
        expect.objectContaining({ kind: 'sab', base_url: 'http://sab:8080', secret: 'APIKEY' })
      )
    );
  });

  it('reveals the username field only for NZBGet', async () => {
    render(DownloaderSettings, { props: { downloader: null } });
    expect(screen.queryByLabelText('Username')).not.toBeInTheDocument();

    await fireEvent.click(screen.getByLabelText('NZBGet'));
    expect(await screen.findByLabelText('Username')).toBeInTheDocument();
  });

  it('runs a connection test and shows the result', async () => {
    vi.mocked(testDownloader).mockResolvedValue({
      ok: false,
      message: 'downloader rejected credentials'
    });
    render(DownloaderSettings, { props: { downloader: sampleDownloader() } });

    await fireEvent.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(await screen.findByText(/rejected credentials/)).toBeInTheDocument();
  });

  it('clears the configuration', async () => {
    vi.mocked(clearDownloader).mockResolvedValue(undefined);
    render(DownloaderSettings, { props: { downloader: sampleDownloader() } });

    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    await waitFor(() => expect(clearDownloader).toHaveBeenCalledTimes(1));
  });

  it('surfaces an ApiError when saving fails', async () => {
    vi.mocked(saveDownloader).mockRejectedValue(
      new ApiError(400, 'bad_request', 'username is required for NZBGet')
    );
    render(DownloaderSettings, { props: { downloader: null } });

    await fireEvent.input(screen.getByLabelText('Base URL'), { target: { value: 'http://x' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText(/username is required/)).toBeInTheDocument();
  });
});
