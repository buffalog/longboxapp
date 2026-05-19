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

// CV's description HTML carries anchors with path-only hrefs
// (`<a href="/absolute-batman/4050-167340/">`). The browser resolves
// those against the current origin and turns the "Collected in" link
// into a broken LongBox-internal route. Rewrite to absolute at render
// time rather than mutate on ingest: the source data stays byte-for-byte
// as CV delivered it, and the single render site stays the single point
// where any fix-up happens.
//
// Only rewrites hrefs that begin with a single `/` followed by anything
// other than `/` — that's the shape CV uses. Already-absolute URLs
// (`http://`, `https://`), protocol-relative (`//cdn…`), fragment
// (`#…`), `mailto:`, and `tel:` are passed through untouched.
const CV_BASE = 'https://comicvine.gamespot.com';
const CV_RELATIVE_HREF_RE = /\bhref\s*=\s*(["'])(\/[^/][^"']*)\1/gi;

export function absolutizeCvLinks(html: string): string {
  if (!html) return '';
  return html.replace(CV_RELATIVE_HREF_RE, (_m, quote, path) => `href=${quote}${CV_BASE}${path}${quote}`);
}
