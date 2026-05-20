import { afterEach, describe, expect, it, vi } from 'vitest';
import { createWebhook, deleteWebhook, listWebhooks, updateWebhook, WEBHOOK_EVENTS } from './webhooks';

describe('webhooks api', () => {
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

  it('WEBHOOK_EVENTS covers four events as distinct power-of-two bits', () => {
    expect(WEBHOOK_EVENTS).toHaveLength(4);
    expect(WEBHOOK_EVENTS.map((e) => e.bit)).toEqual([1, 2, 4, 8]);
  });

  it('listWebhooks GETs /api/webhooks', async () => {
    const fetchSpy = mockFetch([{ id: 1, name: 'Slack' }]);
    const out = await listWebhooks();
    expect(out).toHaveLength(1);
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/webhooks');
  });

  it('createWebhook POSTs the input as a JSON body', async () => {
    const fetchSpy = mockFetch({ id: 1 });
    await createWebhook({ name: 'Slack', url: 'https://x', event_mask: 5, enabled: true });
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/webhooks');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toMatchObject({ name: 'Slack', event_mask: 5 });
  });

  it('updateWebhook PUTs to /api/webhooks/:id', async () => {
    const fetchSpy = mockFetch({ id: 4 });
    await updateWebhook(4, { name: 'Slack', url: 'https://x', event_mask: 1, enabled: false });
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/webhooks/4');
    expect(opts?.method).toBe('PUT');
  });

  it('deleteWebhook DELETEs /api/webhooks/:id', async () => {
    const fetchSpy = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchSpy);
    await deleteWebhook(9);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/webhooks/9');
    expect(opts?.method).toBe('DELETE');
  });
});
