import { afterEach, describe, expect, it, vi } from 'vitest';
import { getPullFailures, retryPull } from './needs_attention';

describe('needs-attention api', () => {
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
});
