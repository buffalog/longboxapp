import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  addFolders,
  bulkDeletePhantoms,
  deletePhantom,
  dismissFolders,
  keepPhantom,
  listPhantoms,
  listUntracked
} from './reconcile';

describe('reconcile api', () => {
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

  it('listPhantoms GETs /api/reconcile/phantoms', async () => {
    const fetchSpy = mockJson({ with_transition: [], all_zero_owned: [] });
    const out = await listPhantoms();
    expect(out.all_zero_owned).toEqual([]);
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/reconcile/phantoms');
  });

  it('listUntracked GETs /api/reconcile/untracked', async () => {
    const fetchSpy = mockJson([]);
    await listUntracked();
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/reconcile/untracked');
  });

  it('addFolders POSTs the folders array', async () => {
    const fetchSpy = mockJson({ succeeded: [], failed: [] });
    await addFolders([{ folder_name: 'Saga (2012)', cv_id: 18000 }]);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/reconcile/add');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({
      folders: [{ folder_name: 'Saga (2012)', cv_id: 18000 }]
    });
  });

  it('dismissFolders POSTs folder_names', async () => {
    const fetchSpy = mockJson({ dismissed: 2 });
    const out = await dismissFolders(['A', 'B']);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/reconcile/dismiss');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({ folder_names: ['A', 'B'] });
    expect(out.dismissed).toBe(2);
  });

  it('deletePhantom DELETEs /api/reconcile/phantom/:id', async () => {
    const fetchSpy = mockJson({ deleted: 7 });
    await deletePhantom(7);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/reconcile/phantom/7');
    expect(opts?.method).toBe('DELETE');
  });

  it('bulkDeletePhantoms POSTs series_ids', async () => {
    const fetchSpy = mockJson({ deleted: [1, 2], skipped: [] });
    await bulkDeletePhantoms([1, 2]);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/reconcile/phantoms/bulk');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({ series_ids: [1, 2] });
  });

  it('keepPhantom POSTs /api/reconcile/phantom/:id/keep', async () => {
    const fetchSpy = mockJson({ kept: 7 });
    await keepPhantom(7);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/reconcile/phantom/7/keep');
    expect(opts?.method).toBe('POST');
  });
});
