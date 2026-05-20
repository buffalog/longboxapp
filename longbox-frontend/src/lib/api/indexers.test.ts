import { afterEach, describe, expect, it, vi } from 'vitest';
import { createIndexer, deleteIndexer, listIndexers, testIndexer, updateIndexer } from './indexers';

describe('indexers api', () => {
  afterEach(() => vi.unstubAllGlobals());

  /** Stub `fetch` with a JSON response; returns the spy for assertions. */
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

  it('listIndexers GETs /api/indexers', async () => {
    const fetchSpy = mockFetch([{ id: 1, name: 'NZBgeek' }]);
    const out = await listIndexers();
    expect(out).toHaveLength(1);
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/indexers');
  });

  it('createIndexer POSTs the input as a JSON body', async () => {
    const fetchSpy = mockFetch({ id: 1 });
    await createIndexer({
      name: 'X',
      base_url: 'https://x',
      api_key: 'K',
      enabled: true,
      priority: 0,
      maxage_days: 1500
    });
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe('/api/indexers');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toMatchObject({ name: 'X', api_key: 'K' });
  });

  it('updateIndexer PUTs to /api/indexers/:id', async () => {
    const fetchSpy = mockFetch({ id: 7 });
    await updateIndexer(7, {
      name: 'X',
      base_url: 'https://x',
      api_key: '',
      enabled: true,
      priority: 1,
      maxage_days: 900
    });
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe('/api/indexers/7');
    expect(opts?.method).toBe('PUT');
  });

  it('deleteIndexer DELETEs /api/indexers/:id', async () => {
    const fetchSpy = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchSpy);
    await deleteIndexer(3);
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe('/api/indexers/3');
    expect(opts?.method).toBe('DELETE');
  });

  it('testIndexer POSTs to /api/indexers/test', async () => {
    const fetchSpy = mockFetch({ ok: true, message: 'good' });
    const out = await testIndexer({ base_url: 'https://x', api_key: 'K' });
    expect(out.ok).toBe(true);
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe('/api/indexers/test');
    expect(opts?.method).toBe('POST');
  });
});
