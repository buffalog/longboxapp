import { describe, it, expect } from 'vitest';
import { absolutizeCvLinks, htmlToPlainText } from './text';

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
