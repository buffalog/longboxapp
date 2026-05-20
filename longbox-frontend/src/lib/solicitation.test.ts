import { describe, it, expect, vi, afterEach } from 'vitest';
import { isSolicited } from './solicitation';

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
    expect(isSolicited('2026-08-01')).toBe(true);
    expect(isSolicited('2027-01-01')).toBe(true);
  });

  it('is false for a past cover date', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited('2026-05-18')).toBe(false);
    expect(isSolicited('2024-01-01')).toBe(false);
  });

  it('is true for a cover date of exactly today (today-inclusive)', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited('2026-05-19')).toBe(true);
  });

  it('is false for null / undefined / empty', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited(null)).toBe(false);
    expect(isSolicited(undefined)).toBe(false);
    expect(isSolicited('')).toBe(false);
  });

  it('is false for unparseable / garbage input', () => {
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited('not-a-date')).toBe(false);
    expect(isSolicited('2026-13-99')).toBe(false);
  });

  it('treats a partial YYYY-only cover date as not solicited', () => {
    // CV occasionally emits partial dates; `YYYY` + 'T00:00:00Z'
    // doesn't parse cleanly, so it falls back to "missing".
    freezeNow('2026-05-19T12:00:00Z');
    expect(isSolicited('2099')).toBe(false);
  });
});
