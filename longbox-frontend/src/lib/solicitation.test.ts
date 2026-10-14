import { describe, it, expect, vi, afterEach } from 'vitest';
import { isSolicited, releaseDate } from './solicitation';

describe('isSolicited', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  /** Pin "now" so date assertions are deterministic. */
  function freezeNow(iso: string): void {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(iso));
  }

  it('is true for a future cover date', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ cover_date: '2026-08-01' })).toBe(true);
    expect(isSolicited({ cover_date: '2027-01-01' })).toBe(true);
  });

  it('is false for a past cover date', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ cover_date: '2026-05-18' })).toBe(false);
    expect(isSolicited({ cover_date: '2024-01-01' })).toBe(false);
  });

  it('is true for a cover date of exactly today (today-inclusive)', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ cover_date: '2026-05-19' })).toBe(true);
  });

  it('is false for null / undefined / empty', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ cover_date: null })).toBe(false);
    expect(isSolicited({ cover_date: null, store_date: undefined })).toBe(false);
    expect(isSolicited({ cover_date: '' })).toBe(false);
  });

  it('is false for unparseable / garbage input', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ cover_date: 'not-a-date' })).toBe(false);
    expect(isSolicited({ cover_date: '2026-13-99' })).toBe(false);
  });

  it('treats a partial YYYY-only cover date as not solicited', () => {
    // CV occasionally emits partial dates; `YYYY` + 'T00:00:00Z'
    // doesn't parse cleanly, so it falls back to "missing".
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ cover_date: '2099' })).toBe(false);
  });

  it('prefers store_date: on sale already, cover date still ahead', () => {
    // CV runs cover_date 1-2 months past on-sale. The issue is on shelves,
    // so it is missing, not solicited.
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ store_date: '2026-05-13', cover_date: '2026-07-01' })).toBe(false);
  });

  it('prefers store_date: not on sale yet', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ store_date: '2026-05-27', cover_date: '2026-05-01' })).toBe(true);
  });

  it('falls back to cover_date when store_date is null', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited({ store_date: null, cover_date: '2026-08-01' })).toBe(true);
  });
});

describe('releaseDate', () => {
  it('is store_date, else cover_date', () => {
    expect(releaseDate({ store_date: '2026-05-13', cover_date: '2026-07-01' })).toBe('2026-05-13');
    expect(releaseDate({ store_date: null, cover_date: '2026-07-01' })).toBe('2026-07-01');
    expect(releaseDate({ cover_date: null })).toBe(null);
  });
});
