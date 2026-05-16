import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, apiFetch } from './client';

declare global {
  // eslint-disable-next-line no-var
  var fetch: typeof global.fetch;
}

describe('apiFetch', () => {
  let originalFetch: typeof global.fetch;

  beforeEach(() => {
    originalFetch = global.fetch;
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });

  it('returns parsed JSON on 200', async () => {
    global.fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ ok: true, n: 42 }), {
        status: 200,
        headers: { 'content-type': 'application/json' }
      })
    );
    const out = await apiFetch<{ ok: boolean; n: number }>('/x');
    expect(out).toEqual({ ok: true, n: 42 });
  });

  it('parses error envelope into ApiError', async () => {
    global.fetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          error: { code: 'conflict.scan_running', message: 'busy', details: {} }
        }),
        { status: 409 }
      )
    );
    await expect(apiFetch('/x')).rejects.toMatchObject({
      status: 409,
      code: 'conflict.scan_running',
      message: 'busy'
    });
  });

  it('uses statusText when body has no error envelope', async () => {
    global.fetch = vi
      .fn()
      .mockResolvedValue(new Response('', { status: 500, statusText: 'boom' }));
    await expect(apiFetch('/x')).rejects.toMatchObject({
      status: 500,
      code: 'unknown'
    });
  });

  it('wraps network failures as ApiError(network)', async () => {
    global.fetch = vi.fn().mockRejectedValue(new TypeError('failed to fetch'));
    await expect(apiFetch('/x')).rejects.toBeInstanceOf(ApiError);
    await expect(apiFetch('/x')).rejects.toMatchObject({ code: 'network' });
  });

  it('handles malformed JSON body gracefully', async () => {
    global.fetch = vi.fn().mockResolvedValue(
      new Response('not json', {
        status: 502,
        headers: { 'content-type': 'application/json' }
      })
    );
    // No throw on parse failure; raw is captured but error envelope is
    // absent, so it falls through to statusText.
    await expect(apiFetch('/x')).rejects.toMatchObject({ status: 502 });
  });
});
