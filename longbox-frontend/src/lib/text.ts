// Plain-text rendering of HTML strings. Used where we want a clean,
// untagged preview of a CV-supplied description without losing block
// boundaries (e.g. the collapsed series-header preview).
//
// Two-pass strip:
//   1. Replace block-closing tags + line breaks with `\n` so paragraph
//      boundaries survive the tag strip below. CV stamps `</p>` between
//      paragraphs without any literal whitespace, so a naive
//      `.replace(/<[^>]*>/g, '')` produces "EndOfOneStartOfTwo".
//   2. Strip remaining tags. Anything not in the block list (`<i>`,
//      `<a>`, `<b>`, etc.) collapses to nothing — we lose inline
//      formatting, which is correct for a plain-text preview.
//
// Then decode the small set of HTML entities CV actually uses. Generic
// numeric entities (`&#1234;`) are intentionally not handled; if a real
// CV fixture ever surfaces one we'll add a fixture-driven test for it.

const BLOCK_TAG_RE =
  /<\s*\/\s*(?:p|h[1-6]|div|li|ul|ol|blockquote|pre)\s*>|<\s*br\s*\/?\s*>/gi;
const ANY_TAG_RE = /<[^>]*>/g;
const MULTI_NEWLINE_RE = /\n{3,}/g;

const ENTITIES: Record<string, string> = {
  '&amp;': '&',
  '&lt;': '<',
  '&gt;': '>',
  '&quot;': '"',
  '&#39;': "'",
  '&apos;': "'",
  '&nbsp;': ' '
};
const ENTITY_RE = /&(?:amp|lt|gt|quot|apos|nbsp|#39);/g;

export function htmlToPlainText(html: string): string {
  if (!html) return '';
  const blockBroken = html.replace(BLOCK_TAG_RE, '\n');
  const tagsStripped = blockBroken.replace(ANY_TAG_RE, '');
  const decoded = tagsStripped.replace(ENTITY_RE, (m) => ENTITIES[m] ?? m);
  return decoded.replace(MULTI_NEWLINE_RE, '\n\n').trim();
}
