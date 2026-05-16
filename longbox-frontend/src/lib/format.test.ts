import { describe, expect, it } from 'vitest';
import {
  formatBytes,
  formatConfidence,
  formatDate,
  formatDateTime,
  formatDuration,
  formatMatchMethod,
  formatRelative
} from './format';

describe('formatConfidence', () => {
  it('renders 0..1 as percentages', () => {
    expect(formatConfidence(0)).toBe('0%');
    expect(formatConfidence(0.5)).toBe('50%');
    expect(formatConfidence(0.85)).toBe('85%');
    expect(formatConfidence(1)).toBe('100%');
  });
  it('rounds half-up', () => {
    expect(formatConfidence(0.847)).toBe('85%');
  });
});

describe('formatMatchMethod', () => {
  it('maps known enum values to friendly labels', () => {
    expect(formatMatchMethod('web_url_cv')).toBe('CV URL');
    expect(formatMatchMethod('web_url_metron')).toBe('Metron URL');
    expect(formatMatchMethod('comicinfo_xml')).toBe('ComicInfo');
    expect(formatMatchMethod('filename_regex')).toBe('Filename');
    expect(formatMatchMethod('manual')).toBe('Manual');
    expect(formatMatchMethod('unmatched')).toBe('Unmatched');
    expect(formatMatchMethod('ignored')).toBe('Ignored');
  });
  it('passes through unknown values', () => {
    expect(formatMatchMethod('something_new')).toBe('something_new');
  });
});

describe('formatDuration', () => {
  it('handles sub-second', () => {
    expect(formatDuration(500)).toBe('500ms');
  });
  it('handles seconds', () => {
    expect(formatDuration(2500)).toBe('2s');
  });
  it('handles minutes and seconds', () => {
    expect(formatDuration(125_000)).toBe('2m 5s');
  });
  it('handles whole minutes', () => {
    expect(formatDuration(120_000)).toBe('2m');
  });
  it('handles hours', () => {
    expect(formatDuration(3_700_000)).toBe('1h 1m');
    expect(formatDuration(7_200_000)).toBe('2h');
  });
});

describe('formatBytes', () => {
  it('renders units sensibly', () => {
    expect(formatBytes(500)).toBe('500 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(3 * 1024 * 1024)).toBe('3.0 MB');
    expect(formatBytes(2 * 1024 * 1024 * 1024)).toBe('2.00 GB');
  });
});

describe('formatDateTime / formatDate', () => {
  it('formats ISO timestamps to UTC short form', () => {
    expect(formatDateTime('2026-05-16T12:34:56Z')).toBe('2026-05-16 12:34');
  });
  it('returns dash for null', () => {
    expect(formatDateTime(null)).toBe('—');
    expect(formatDate(null)).toBe('—');
  });
  it('passes through unparseable input', () => {
    expect(formatDateTime('not a date')).toBe('not a date');
  });
});

describe('formatRelative', () => {
  it('handles seconds-old timestamps', () => {
    const fiveSec = new Date(Date.now() - 5000).toISOString();
    expect(formatRelative(fiveSec)).toMatch(/^\ds ago$/);
  });
  it('returns dash for null', () => {
    expect(formatRelative(null)).toBe('—');
  });
  it('falls back to absolute for future timestamps', () => {
    const future = new Date(Date.now() + 60_000).toISOString();
    expect(formatRelative(future)).toMatch(/^\d{4}-/);
  });
});
