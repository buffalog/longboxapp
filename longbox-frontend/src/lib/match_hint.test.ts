import { describe, it, expect } from 'vitest';
import { searchHintFromPath } from './match_hint';

describe('searchHintFromPath', () => {
  it('uses parent directory name with year stripped', () => {
    expect(searchHintFromPath('Saga (2012)/Saga 001.cbz')).toBe('Saga');
  });

  it('handles deeper trees by picking the nearest parent', () => {
    expect(searchHintFromPath('Image/Saga (2012)/Saga 042.cbz')).toBe('Saga');
  });

  it('handles year-range suffix', () => {
    expect(searchHintFromPath('Y The Last Man (2002-2008)/Y 001.cbz')).toBe('Y The Last Man');
  });

  it('handles open-ended year range', () => {
    expect(searchHintFromPath('Saga (2012- )/Saga 001.cbz')).toBe('Saga');
  });

  it('skips a basename-only path and falls back to the basename', () => {
    expect(searchHintFromPath('Saga (2012) 001.cbz')).toBe('Saga (2012) 001');
  });

  it('returns empty for empty input', () => {
    expect(searchHintFromPath('')).toBe('');
  });

  it('walks up when the immediate parent is empty after stripping', () => {
    // The "Comics" parent is fine, no year suffix to strip.
    expect(searchHintFromPath('Comics/Saga 001.cbz')).toBe('Comics');
  });

  it('keeps non-year parenthetical groups untouched', () => {
    expect(searchHintFromPath('Saga (Director\'s Cut)/Saga 001.cbz')).toBe(
      "Saga (Director's Cut)"
    );
  });
});
