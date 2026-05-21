import { afterEach, describe, expect, it, vi } from 'vitest';
import { addCalendarVolumeToPullList, getReleaseCalendar, getReleasesOfNote } from './releases';

describe('releases api', () => {
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

  it('getReleaseCalendar GETs the calendar with from/to params', async () => {
    const fetchSpy = mockJson([]);
    await getReleaseCalendar('2026-05-13', '2026-05-19');
    expect(fetchSpy.mock.calls[0]![0]).toBe(
      '/api/releases/calendar?from=2026-05-13&to=2026-05-19'
    );
  });

  it('getReleaseCalendar adds refresh=true when forced', async () => {
    const fetchSpy = mockJson([]);
    await getReleaseCalendar('2026-05-13', '2026-05-19', true);
    expect(fetchSpy.mock.calls[0]![0]).toBe(
      '/api/releases/calendar?from=2026-05-13&to=2026-05-19&refresh=true'
    );
  });

  it('addCalendarVolumeToPullList POSTs the cv_volume_id', async () => {
    const fetchSpy = mockJson({ series_id: 42 });
    const out = await addCalendarVolumeToPullList(2127);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/releases/calendar/pull');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({ cv_volume_id: 2127 });
    expect(out.series_id).toBe(42);
  });

  it('getReleasesOfNote GETs /api/releases/of-note', async () => {
    const fetchSpy = mockJson([]);
    await getReleasesOfNote();
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/releases/of-note');
  });
});
