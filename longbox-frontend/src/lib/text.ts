// Helpers for rendering CV-supplied HTML content. Three flavors:
//
//  - htmlToPlainText: strip-to-text with block-boundary preservation
//    (used for collapsed previews).
//  - absolutizeCvLinks: rewrite path-only anchor hrefs to absolute
//    ComicVine URLs (CV's description HTML uses path-only hrefs that
//    otherwise resolve against LongBox's origin).
//  - sanitizeCvSynopsis: absolutize + DOMPurify pipeline for live
//    `{@html}` rendering of per-issue synopsis HTML. The composition
//    order matters: absolutize first (does nothing structural, just
//    rewrites href values), then sanitize (the canonical XSS guard).
//
// Plain-text helper's strip is a two-pass strip:
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

import DOMPurify from 'dompurify';

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

// Strict allow-list of tags CV's per-issue synopsis HTML actually
// uses, plus tags useful for the text formatting it does. DOMPurify
// defaults already strip <script>, event handlers, javascript: URLs,
// and similar; this allow-list is an extra safety net so an unexpected
// tag from CV doesn't slip through unnoticed.
const SYNOPSIS_ALLOWED_TAGS = [
  'a', 'b', 'br', 'em', 'i', 'strong', 'u',
  'p', 'span', 'div',
  'ul', 'ol', 'li',
  'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
  'blockquote', 'pre', 'code',
];
const SYNOPSIS_ALLOWED_ATTRS = ['href', 'title', 'target', 'rel'];

/// Sanitize a CV-supplied synopsis HTML string for live `{@html}`
/// rendering. Pipeline: absolutize relative hrefs first (so anchors
/// point at comicvine.gamespot.com, not LongBox's origin), then run
/// DOMPurify with the strict tag/attr allow-list above.
///
/// Forces `target="_blank"` + `rel="noopener noreferrer"` on anchors
/// that survive sanitization so external CV navigation opens in a
/// new tab and the linked page can't reach back into LongBox via
/// `window.opener`.
export function sanitizeCvSynopsis(html: string): string {
  if (!html) return '';
  const absolutized = absolutizeCvLinks(html);
  const sanitized = DOMPurify.sanitize(absolutized, {
    ALLOWED_TAGS: SYNOPSIS_ALLOWED_TAGS,
    ALLOWED_ATTR: SYNOPSIS_ALLOWED_ATTRS,
  });
  // DOMPurify won't add attributes by itself; post-process to force
  // safe external-link behavior on every surviving anchor.
  return sanitized.replace(
    /<a\s+([^>]*?)>/gi,
    (_m, attrs) => {
      const cleaned = attrs
        .replace(/\s*\btarget\s*=\s*["'][^"']*["']/gi, '')
        .replace(/\s*\brel\s*=\s*["'][^"']*["']/gi, '')
        .trim();
      const prefix = cleaned.length ? cleaned + ' ' : '';
      return `<a ${prefix}target="_blank" rel="noopener noreferrer">`;
    }
  );
}
