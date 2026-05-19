import { describe, it, expect } from 'vitest';
import { absolutizeCvLinks, htmlToPlainText, sanitizeCvSynopsis } from './text';

describe('htmlToPlainText', () => {
  it('inserts a line break between adjacent <p> tags', () => {
    // Each block-closer is replaced with a single \n (per spec). Adjacent
    // </p><p> becomes one \n, not two. CSS `whitespace-pre-line` renders
    // these as visible line breaks; the closeout's smushed-output bug is
    // gone either way.
    expect(htmlToPlainText('<p>Editions</p><p>Marvel Universe by Frank Miller Omnibus</p>')).toBe(
      'Editions\nMarvel Universe by Frank Miller Omnibus'
    );
  });

  it('breaks on heading and list closers as well', () => {
    expect(
      htmlToPlainText(
        '<h2>Collected Editions</h2><ul><li>Marvel Universe by Frank Miller Omnibus</li><li>Uncanny X-Men Omnibus Volume 3</li></ul>'
      )
    ).toBe(
      'Collected Editions\nMarvel Universe by Frank Miller Omnibus\nUncanny X-Men Omnibus Volume 3'
    );
  });

  it('honors <br> and self-closing variants', () => {
    expect(htmlToPlainText('Line one<br>Line two<br/>Line three<br />Line four')).toBe(
      'Line one\nLine two\nLine three\nLine four'
    );
  });

  it('strips inline tags without breaking words', () => {
    expect(htmlToPlainText('<p>Read about <i>Wolverine</i> in <b>1982</b>.</p>')).toBe(
      'Read about Wolverine in 1982.'
    );
  });

  it('collapses 3+ blank lines to 2', () => {
    expect(htmlToPlainText('<p>A</p><br><br><br><p>B</p>')).toBe('A\n\nB');
  });

  it('decodes the entities CV actually emits', () => {
    expect(
      htmlToPlainText(
        '<p>Tony &amp; Bucky&#39;s &quot;rivalry&quot; spans 1&lt;2 issues&nbsp;total.</p>'
      )
    ).toBe('Tony & Bucky\'s "rivalry" spans 1<2 issues total.');
  });

  it('trims trailing and leading whitespace', () => {
    expect(htmlToPlainText('   <p>Hello</p>   ')).toBe('Hello');
  });

  it('returns empty string for empty input', () => {
    expect(htmlToPlainText('')).toBe('');
  });

  // The kind of input that broke SeriesHeader's collapsed preview before
  // Task D: paragraphs of running prose with no whitespace between
  // `</p><p>` boundaries.
  it('renders a realistic CV-style multi-paragraph description cleanly', () => {
    const input =
      '<p>The first Wolverine miniseries was published by Marvel in September of 1982 ' +
      'and ran for four issues.</p>' +
      '<p>It is widely cited as a turning point for the character: Frank Miller and Chris Claremont ' +
      'sent Logan to Japan, established his "I\'m the best there is at what I do" voice, and laid ' +
      'the groundwork for the romance with Mariko Yashida.</p>' +
      '<h3>Collected Editions</h3>' +
      '<ul><li>Marvel Universe by Frank Miller Omnibus</li>' +
      '<li>Uncanny X-Men Omnibus Volume 3</li></ul>';
    const out = htmlToPlainText(input);
    expect(out).toContain('Wolverine miniseries');
    expect(out).toContain('the best there is at what I do');
    expect(out).toContain('Collected Editions');
    expect(out).not.toContain('miniseries.It');
    expect(out).not.toContain('OmnibusUncanny');
    // No HTML tags left.
    expect(out).not.toMatch(/<[^>]+>/);
    // No more than two consecutive newlines anywhere.
    expect(out).not.toMatch(/\n{3,}/);
  });
});

describe('absolutizeCvLinks', () => {
  it('prepends the CV origin to path-only hrefs', () => {
    expect(
      absolutizeCvLinks('<a href="/absolute-batman/4050-167340/">Absolute Batman</a>')
    ).toBe(
      '<a href="https://comicvine.gamespot.com/absolute-batman/4050-167340/">Absolute Batman</a>'
    );
  });

  it('handles single-quoted hrefs', () => {
    expect(absolutizeCvLinks("<a href='/foo/123/'>x</a>")).toBe(
      "<a href='https://comicvine.gamespot.com/foo/123/'>x</a>"
    );
  });

  it('leaves already-absolute hrefs untouched', () => {
    const input = '<a href="https://example.com/foo">x</a>';
    expect(absolutizeCvLinks(input)).toBe(input);
  });

  it('leaves protocol-relative, fragment, mailto, tel hrefs untouched', () => {
    const input =
      '<a href="//cdn.example.com/img">a</a>' +
      '<a href="#top">b</a>' +
      '<a href="mailto:x@y.com">c</a>' +
      '<a href="tel:+15551234">d</a>';
    expect(absolutizeCvLinks(input)).toBe(input);
  });

  it('rewrites every relative href in a CV-style description', () => {
    const input =
      '<p>Featured in <a href="/absolute-batman/4050-167340/">Absolute Batman</a> ' +
      'and <a href="/dark-knight/4050-12345/">The Dark Knight</a>.</p>';
    const out = absolutizeCvLinks(input);
    expect(out).toContain('https://comicvine.gamespot.com/absolute-batman/4050-167340/');
    expect(out).toContain('https://comicvine.gamespot.com/dark-knight/4050-12345/');
    // Verify no remaining bare-relative hrefs.
    expect(out).not.toMatch(/href=["']\/[^/]/);
  });

  it('returns empty string for empty input', () => {
    expect(absolutizeCvLinks('')).toBe('');
  });
});

describe('sanitizeCvSynopsis', () => {
  it('preserves benign CV-style markup', () => {
    const out = sanitizeCvSynopsis(
      '<p>Wolverine returns. <i>Logan</i> faces <b>Sabretooth</b>.</p>' +
      '<ul><li>Issue 1: arrival</li><li>Issue 2: confrontation</li></ul>'
    );
    expect(out).toContain('<p>');
    expect(out).toContain('<i>Logan</i>');
    expect(out).toContain('<b>Sabretooth</b>');
    expect(out).toContain('<ul>');
    expect(out).toContain('<li>Issue 1: arrival</li>');
  });

  it('strips <script> tags entirely', () => {
    const out = sanitizeCvSynopsis(
      '<p>Hello</p><script>alert("xss")</script><p>World</p>'
    );
    expect(out).not.toContain('<script>');
    expect(out).not.toContain('alert');
    expect(out).toContain('Hello');
    expect(out).toContain('World');
  });

  it('strips event handler attributes (onerror, onclick, etc.)', () => {
    const out = sanitizeCvSynopsis(
      '<p onclick="alert(1)">click me</p><img src=x onerror="alert(2)">'
    );
    expect(out).not.toMatch(/onclick/i);
    expect(out).not.toMatch(/onerror/i);
    expect(out).not.toMatch(/alert/);
  });

  it('strips javascript: URLs from hrefs', () => {
    const out = sanitizeCvSynopsis('<a href="javascript:alert(1)">click</a>');
    expect(out).not.toMatch(/javascript:/i);
    expect(out).not.toMatch(/alert/);
  });

  it('absolutizes relative anchor hrefs before sanitizing', () => {
    const out = sanitizeCvSynopsis('<p>See <a href="/wolverine/4050-1234/">vol</a></p>');
    expect(out).toContain('https://comicvine.gamespot.com/wolverine/4050-1234/');
  });

  it('forces target=_blank rel=noopener noreferrer on every anchor', () => {
    const out = sanitizeCvSynopsis(
      '<a href="https://comicvine.gamespot.com/x">A</a>' +
      '<a href="/y/4050-1/">B</a>'
    );
    // Every anchor that survives sanitization must have safe target+rel.
    const anchors = out.match(/<a[^>]*>/gi) ?? [];
    expect(anchors.length).toBe(2);
    for (const a of anchors) {
      expect(a).toMatch(/target="_blank"/);
      expect(a).toMatch(/rel="noopener noreferrer"/);
    }
  });

  it('replaces existing target / rel attrs rather than duplicating', () => {
    const out = sanitizeCvSynopsis(
      '<a href="/x/4050-1/" target="_self" rel="opener">B</a>'
    );
    expect(out).not.toMatch(/target="_self"/);
    expect(out).not.toMatch(/rel="opener"/);
    expect(out).toMatch(/target="_blank"/);
    expect(out).toMatch(/rel="noopener noreferrer"/);
    // And only one of each, no duplicate attribute.
    expect((out.match(/target=/g) ?? []).length).toBe(1);
    expect((out.match(/rel=/g) ?? []).length).toBe(1);
  });

  it('drops unexpected tags not on the allow-list', () => {
    const out = sanitizeCvSynopsis(
      '<p>Hello</p><iframe src="https://evil.com"></iframe><object data="x"></object>'
    );
    expect(out).not.toContain('<iframe');
    expect(out).not.toContain('<object');
    expect(out).toContain('Hello');
  });

  it('returns empty string for empty input', () => {
    expect(sanitizeCvSynopsis('')).toBe('');
  });
});
