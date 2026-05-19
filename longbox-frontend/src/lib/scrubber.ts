// Alphabetical scrubber helpers. Used by `AlphaScrubber.svelte` and
// any caller that needs to bucket a sort-key string by its leading
// letter for an iOS-contacts-style jump index.
//
// Bucket rule: take the first non-whitespace character of the sort key
// and uppercase it. ASCII A-Z is its own bucket. Everything else (digits,
// symbols, accented characters that don't fold to A-Z) lands in the `#`
// bucket, which renders AFTER Z visually per the iOS contacts pattern.
// Empty / whitespace-only keys also map to `#`.

/** Bucket letters in render order: A through Z, then `#`. */
export const ALPHA_BUCKETS: readonly string[] = [
  ...Array.from({ length: 26 }, (_, i) => String.fromCharCode(65 + i)),
  '#',
];

/** Compute the bucket letter for a sort-key string. */
export function bucketLetter(sortKey: string): string {
  const trimmed = sortKey.trim();
  if (trimmed.length === 0) return '#';
  const ch = trimmed.charAt(0).toUpperCase();
  return ch >= 'A' && ch <= 'Z' ? ch : '#';
}
