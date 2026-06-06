// Component test for the Settings page's editable Tunable settings
// section. Locks the per-row save flow: an input change + Save click
// PUTs the right key with the right value, and the success path
// re-invalidates the page load so the next render reflects the
// canonical server value.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Settings } from '$lib/api/settings';
import { updateSetting } from '$lib/api/settings';
import { invalidateAll } from '$app/navigation';
import Page from './+page.svelte';

vi.mock('$lib/api/settings', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/settings')>()),
  updateSetting: vi.fn()
}));

// The page imports a handful of other API surfaces it doesn't exercise
// in these tests; mock them to no-ops so the component renders.
vi.mock('$lib/api/admin', () => ({ restartServer: vi.fn() }));
vi.mock('$lib/api/publishers', () => ({
  addFilter: vi.fn(),
  deleteFilter: vi.fn(),
  resetFiltersToDefaults: vi.fn()
}));

vi.mock('$app/navigation', () => ({
  invalidateAll: vi.fn(),
  goto: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

function settings(over: Partial<Settings> = {}): Settings {
  return {
    library_root_path: '/library',
    database_url: 'sqlite:./longbox.db',
    bind_address: '0.0.0.0:3000',
    log_level: 'info',
    match_threshold: 0.85,
    comicvine_api_key_configured: true,
    download_watch_path: null,
    version: '0.0.1',
    match_confidence_threshold: 0.85,
    pull_indexer_match_threshold: 0.75,
    cv_enrichment_title_threshold_year_known: 0.85,
    cv_enrichment_title_threshold_year_unknown: 0.95,
    pull_exclusion_keywords: '',
    host_library_path: '',
    min_file_size_mb: 35,
    ...over
  };
}

function pageData(over: Partial<Settings> = {}) {
  return {
    props: {
      data: {
        settings: settings(over),
        publisherFilters: [],
        indexers: [],
        downloader: null,
        webhooks: []
      }
    }
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('Settings page — Tunable settings', () => {
  it('renders the six tunables seeded from server values', () => {
    render(
      Page,
      pageData({
        match_confidence_threshold: 0.8,
        pull_indexer_match_threshold: 0.72,
        cv_enrichment_title_threshold_year_known: 0.7,
        cv_enrichment_title_threshold_year_unknown: 0.99,
        pull_exclusion_keywords: 'infinity comic',
        min_file_size_mb: 50
      })
    );

    // Inputs are type="text" with inputmode="decimal" (numeric inputs
    // coerce bound values to a Number, which broke the validator).
    // String comparisons here, not numeric ones.
    expect(
      screen.getByLabelText('Match confidence threshold value')
    ).toHaveValue('0.8');
    expect(
      screen.getByLabelText('Pull indexer match threshold value')
    ).toHaveValue('0.72');
    expect(
      screen.getByLabelText('Enrichment title threshold (year known) value')
    ).toHaveValue('0.7');
    expect(
      screen.getByLabelText('Enrichment title threshold (year unknown) value')
    ).toHaveValue('0.99');
    expect(screen.getByLabelText('Pull exclusion keywords value')).toHaveValue(
      'infinity comic'
    );
    expect(screen.getByLabelText('Minimum file size MB value')).toHaveValue('50');
  });

  it('Save on match threshold PUTs the new value and invalidates the page', async () => {
    vi.mocked(updateSetting).mockResolvedValue({
      key: 'match_confidence_threshold',
      value: '0.5'
    });
    render(Page, pageData());

    const input = screen.getByLabelText('Match confidence threshold value');
    await fireEvent.input(input, { target: { value: '0.5' } });
    // Each tunable is its own form; submit the one whose input we just
    // changed by clicking its Save button.
    const saveButtons = screen.getAllByRole('button', { name: 'Save' });
    await fireEvent.click(saveButtons[0]!);

    await waitFor(() =>
      expect(updateSetting).toHaveBeenCalledWith('match_confidence_threshold', '0.5')
    );
    await waitFor(() => expect(invalidateAll).toHaveBeenCalled());
  });

  it('client-side rejects out-of-range threshold without hitting the wire', async () => {
    render(Page, pageData());
    const input = screen.getByLabelText('Match confidence threshold value');
    await fireEvent.input(input, { target: { value: '5' } });
    const saveButtons = screen.getAllByRole('button', { name: 'Save' });
    await fireEvent.click(saveButtons[0]!);

    expect(updateSetting).not.toHaveBeenCalled();
    const { toast } = await import('$lib/stores/toast.svelte');
    expect(toast.warning).toHaveBeenCalledWith(expect.stringContaining('0.0 and 1.0'));
  });

  it('host library path saves verbatim without client-side validation', async () => {
    // The container can't introspect the host filesystem, so there's
    // no client-side check beyond "what the user typed lands at the
    // server." The backend trims whitespace and stores as-is.
    vi.mocked(updateSetting).mockResolvedValue({
      key: 'host_library_path',
      value: '/Volumes/Comics'
    });
    render(Page, pageData());

    const input = screen.getByLabelText('Host library path value');
    await fireEvent.input(input, { target: { value: '/Volumes/Comics' } });
    // 6th (last) tunable → last Save button. Order: match, pull
    // indexer, enrich-known, enrich-unknown, pull keywords, host path.
    const saveButtons = screen.getAllByRole('button', { name: 'Save' });
    await fireEvent.click(saveButtons[5]!);

    await waitFor(() =>
      expect(updateSetting).toHaveBeenCalledWith('host_library_path', '/Volumes/Comics')
    );
  });

  it('keywords field saves the raw CSV without client-side validation', async () => {
    vi.mocked(updateSetting).mockResolvedValue({
      key: 'pull_exclusion_keywords',
      value: 'a, b, c'
    });
    render(Page, pageData());

    const input = screen.getByLabelText('Pull exclusion keywords value');
    await fireEvent.input(input, { target: { value: 'a, b, c' } });
    // The keywords row is the 5th tunable → 5th Save button.
    const saveButtons = screen.getAllByRole('button', { name: 'Save' });
    await fireEvent.click(saveButtons[4]!);

    await waitFor(() =>
      expect(updateSetting).toHaveBeenCalledWith('pull_exclusion_keywords', 'a, b, c')
    );
  });

  it('min_file_size_mb saves an integer MB count', async () => {
    vi.mocked(updateSetting).mockResolvedValue({
      key: 'min_file_size_mb',
      value: '50'
    });
    render(Page, pageData());

    const input = screen.getByLabelText('Minimum file size MB value');
    await fireEvent.input(input, { target: { value: '50' } });
    // Order: match, pull indexer, enrich-known, enrich-unknown,
    // keywords, host path, min file size → 7th Save button.
    const saveButtons = screen.getAllByRole('button', { name: 'Save' });
    await fireEvent.click(saveButtons[6]!);

    await waitFor(() =>
      expect(updateSetting).toHaveBeenCalledWith('min_file_size_mb', '50')
    );
  });

  it('min_file_size_mb rejects fractional values client-side', async () => {
    render(Page, pageData());
    const input = screen.getByLabelText('Minimum file size MB value');
    await fireEvent.input(input, { target: { value: '35.5' } });
    const saveButtons = screen.getAllByRole('button', { name: 'Save' });
    await fireEvent.click(saveButtons[6]!);

    expect(updateSetting).not.toHaveBeenCalled();
    const { toast } = await import('$lib/stores/toast.svelte');
    expect(toast.warning).toHaveBeenCalledWith(
      expect.stringContaining('whole number')
    );
  });
});
