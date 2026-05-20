import { afterEach, describe, expect, it, vi } from 'vitest';
import { clearDownloader, getDownloader, saveDownloader, testDownloader } from './downloader';

describe('downloader api', () => {
  afterEach(() => vi.unstubAllGlobals());

  function mockFetch(body: unknown, status = 200) {
    const fn = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' }
      })
    );
    vi.stubGlobal('fetch', fn);
    return fn;
  }

  it('getDownloader GETs /api/downloader and passes through a null config', async () => {
    const fetchSpy = mockFetch(null);
    const out = await getDownloader();
    expect(out).toBeNull();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/downloader');
  });

  it('saveDownloader PUTs the input as a JSON body', async () => {
    const fetchSpy = mockFetch({ kind: 'sab' });
    await saveDownloader({
      kind: 'sab',
      base_url: 'http://x',
      username: null,
      secret: 'K',
      category: '',
      enabled: true
    });
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe('/api/downloader');
    expect(opts?.method).toBe('PUT');
    expect(JSON.parse(opts?.body as string)).toMatchObject({ kind: 'sab', secret: 'K' });
  });

  it('clearDownloader DELETEs /api/downloader', async () => {
    const fetchSpy = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchSpy);
    await clearDownloader();
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe('/api/downloader');
    expect(opts?.method).toBe('DELETE');
  });

  it('testDownloader POSTs to /api/downloader/test', async () => {
    const fetchSpy = mockFetch({ ok: false, message: 'bad key' });
    const out = await testDownloader({
      kind: 'sab',
      base_url: 'http://x',
      username: null,
      secret: 'K',
      category: '',
      enabled: true
    });
    expect(out.ok).toBe(false);
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/downloader/test');
  });
});
