import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  clearAllPullFailures,
  dismissPullFailure,
  getPullFailures,
  retryPull
} from './needs_attention';

describe('needs-attention api', () => {
  afterEach(() => vi.unstubAllGlobals());

  function mockJson(body: unknown, status = 200) {
    // A 204 response cannot carry a body (Response constructor rejects
    // it). Build an empty-body Response for those, JSON otherwise.
    const init: ResponseInit = {
      status,
      headers: { 'content-type': 'application/json' }
    };
    const r =
      status === 204 || body === null
        ? new Response(null, init)
        : new Response(JSON.stringify(body), init);
    const fn = vi.fn().mockResolvedValue(r);
    vi.stubGlobal('fetch', fn);
    return fn;
  }

  it('getPullFailures GETs /api/needs-attention/pull-failures', async () => {
    const fetchSpy = mockJson([]);
    await getPullFailures();
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/needs-attention/pull-failures');
  });

  it('retryPull POSTs the series and issue ids', async () => {
    const fetchSpy = mockJson({ cleared: 1 });
    await retryPull(3, 7);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/needs-attention/retry');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({ series_id: 3, issue_id: 7 });
  });

  it('dismissPullFailure DELETEs the by-id endpoint', async () => {
    const fetchSpy = mockJson(null, 204);
    await dismissPullFailure(42);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/needs-attention/pull-failures/42');
    expect(opts?.method).toBe('DELETE');
  });

  it('clearAllPullFailures DELETEs the collection endpoint', async () => {
    const fetchSpy = mockJson(null, 204);
    await clearAllPullFailures();
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/needs-attention/pull-failures');
    expect(opts?.method).toBe('DELETE');
  });
});
