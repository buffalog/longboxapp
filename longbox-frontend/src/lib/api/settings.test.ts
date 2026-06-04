import { afterEach, describe, expect, it, vi } from 'vitest';
import { getSettings, updateSetting } from './settings';

describe('settings api', () => {
  afterEach(() => vi.unstubAllGlobals());

  function mockJson(body: unknown, status = 200) {
    const fn = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' }
      })
    );
    vi.stubGlobal('fetch', fn);
    return fn;
  }

  it('getSettings GETs /api/settings', async () => {
    const fetchSpy = mockJson({});
    await getSettings();
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/settings');
  });

  it('updateSetting PUTs the value in a `{value}` envelope', async () => {
    const fetchSpy = mockJson({ key: 'match_confidence_threshold', value: '0.7' });
    await updateSetting('match_confidence_threshold', '0.7');
    const [url, opts] = fetchSpy.mock.calls[0]!;
    expect(url).toBe('/api/settings/match_confidence_threshold');
    expect(opts?.method).toBe('PUT');
    expect(JSON.parse(opts?.body as string)).toEqual({ value: '0.7' });
  });

  it('updateSetting URL-encodes nothing for the simple keys', async () => {
    // The whitelisted keys are all simple identifiers — no special
    // chars to encode. Belt-and-braces assertion against accidental
    // template-literal weirdness.
    const fetchSpy = mockJson({ key: 'pull_exclusion_keywords', value: 'a,b' });
    await updateSetting('pull_exclusion_keywords', 'a,b');
    expect(fetchSpy.mock.calls[0]![0]).toBe('/api/settings/pull_exclusion_keywords');
  });
});
