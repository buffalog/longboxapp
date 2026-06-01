import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  addCalendarVolumeToPullList,
  bulkAddCalendarVolumesToPullList,
  getReleaseCalendar,
  getReleasesOfNote,
  getThisWeeksPulls
} from './releases';

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
    const out = await addCalendarVolumeToPullList({ cv_volume_id: 2127 });
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/releases/calendar/pull');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({ cv_volume_id: 2127 });
    expect(out.series_id).toBe(42);
  });

  it('addCalendarVolumeToPullList POSTs metron_series_id when present', async () => {
    const fetchSpy = mockJson({ series_id: 99 });
    const out = await addCalendarVolumeToPullList({ metron_series_id: 12345 });
    const [, opts] = fetchSpy.mock.calls[0]!;
    expect(JSON.parse(opts?.body as string)).toEqual({ metron_series_id: 12345 });
    expect(out.series_id).toBe(99);
  });

  it('bulkAddCalendarVolumesToPullList POSTs an items array of mixed targets', async () => {
    const fetchSpy = mockJson({ results: [] });
    await bulkAddCalendarVolumesToPullList([
      { cv_volume_id: 11 },
      { metron_series_id: 22 }
    ]);
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/releases/calendar/pull/bulk');
    expect(opts?.method).toBe('POST');
    expect(JSON.parse(opts?.body as string)).toEqual({
      items: [{ cv_volume_id: 11 }, { metron_series_id: 22 }]
    });
  });

  it('getReleasesOfNote GETs /api/releases/of-note', async () => {
    const fetchSpy = mockJson([]);
    await getReleasesOfNote();
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/releases/of-note');
  });

  it('getThisWeeksPulls GETs /api/releases/this-weeks-pulls', async () => {
    const fetchSpy = mockJson([]);
    await getThisWeeksPulls();
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/releases/this-weeks-pulls');
  });
});
