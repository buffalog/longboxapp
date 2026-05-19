import { describe, expect, it } from 'vitest';
import { ALPHA_BUCKETS, bucketLetter } from './scrubber';

describe('ALPHA_BUCKETS', () => {
  it('renders A-Z then # (27 entries, # last)', () => {
    expect(ALPHA_BUCKETS.length).toBe(27);
    expect(ALPHA_BUCKETS[0]).toBe('A');
    expect(ALPHA_BUCKETS[25]).toBe('Z');
    expect(ALPHA_BUCKETS[26]).toBe('#');
  });
});

describe('bucketLetter', () => {
  it('uppercases ASCII letters to their bucket', () => {
    expect(bucketLetter('apple')).toBe('A');
    expect(bucketLetter('Saga')).toBe('S');
    expect(bucketLetter('zzz')).toBe('Z');
  });

  it('strips leading whitespace before bucketing', () => {
    expect(bucketLetter('  hello')).toBe('H');
    expect(bucketLetter('\t Saga')).toBe('S');
  });

  it('routes digits and symbols to #', () => {
    expect(bucketLetter('1776')).toBe('#');
    expect(bucketLetter('300')).toBe('#');
    expect(bucketLetter('!exclaim')).toBe('#');
    expect(bucketLetter('.dotfile')).toBe('#');
  });

  it('routes accented characters that do not fold to A-Z to #', () => {
    // No Unicode-folding rabbit hole — accented leading chars bucket #.
    expect(bucketLetter('Élan')).toBe('#');
    expect(bucketLetter('Über')).toBe('#');
  });

  it('routes empty + whitespace-only keys to #', () => {
    expect(bucketLetter('')).toBe('#');
    expect(bucketLetter('   ')).toBe('#');
  });
});
