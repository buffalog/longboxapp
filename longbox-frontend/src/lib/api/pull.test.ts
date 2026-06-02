import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  addToPullList,
  checkPull,
  getPullEntry,
  listPullList,
  removeFromPullList,
  searchSeriesNow,
  setPullPaused
} from './pull';

describe('pull api', () => {
  afterEach(() => vi.unstubAllGlobals());

  function mockJson(body: unknown, status = 200) {
    const fn = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' }
      })
    );
    vi.stubGlobal('fetch', fn);
    return fn;
  }

  it('listPullList GETs /api/pull-list', async () => {
    const fetchSpy = mockJson([{ series_id: 1 }]);
    const out = await listPullList();
    expect(out).toHaveLength(1);
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/pull-list');
  });

  it('getPullEntry passes through a null (unsubscribed) response', async () => {
    mockJson(null);
    expect(await getPullEntry(7)).toBeNull();
  });

  it('addToPullList POSTs the series id', async () => {
    const fetchSpy = mockJson({ series_id: 7 });
    await addToPullList(7);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/pull-list');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({ series_id: 7 });
  });

  it('setPullPaused PATCHes /api/pull-list/:id', async () => {
    const fetchSpy = mockJson({ series_id: 7, paused: true });
    await setPullPaused(7, true);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/pull-list/7');
    expect(opts?.method).toBe('PATCH');
    expect(JSON.parse(opts?.body as string)).toEqual({ paused: true });
  });

  it('removeFromPullList DELETEs /api/pull-list/:id', async () => {
    const fetchSpy = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchSpy);
    await removeFromPullList(7);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/pull-list/7');
    expect(opts?.method).toBe('DELETE');
  });

  it('checkPull POSTs /api/pull/check', async () => {
    const fetchSpy = vi.fn().mockResolvedValue(new Response(null, { status: 202 }));
    vi.stubGlobal('fetch', fetchSpy);
    await checkPull();
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/pull/check');
    expect(opts?.method).toBe('POST');
  });

  it('searchSeriesNow POSTs /api/pull/search/:series_id', async () => {
    const fetchSpy = vi.fn().mockResolvedValue(new Response(null, { status: 202 }));
    vi.stubGlobal('fetch', fetchSpy);
    await searchSeriesNow(42);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/pull/search/42');
    expect(opts?.method).toBe('POST');
  });
});
