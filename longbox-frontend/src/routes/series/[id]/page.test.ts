// Component test for the series detail page. Locks the cv_id-aware
// rendering: a CV-linked series shows Refresh + the "Hit Refresh"
// hint; a shallow series (cv_id null) hides Refresh and shows the
// scan-attaches-files hint instead. Plugs the test-coverage gap that
// let the shallow-Refresh-400 bug ship in the first place — F4 from
// the hot-fix kickoff (the page had no tests, so the conditional
// branch was never exercised either way).
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SeriesDetail, SeriesSearchResult } from '$lib/types';
import type { PullEntry } from '$lib/api/pull';
import { deleteSeries } from '$lib/api/series';
import { setSeriesCvId } from '$lib/api/enrichment';
import { searchVolumes } from '$lib/api/cv';
import { invalidateAll } from '$app/navigation';
import { ApiError } from '$lib/api/client';
import Page from './+page.svelte';

vi.mock('$lib/api/series', () => ({
  refreshSeries: vi.fn(),
  deleteSeries: vi.fn(),
  getSeriesFolderPath: vi.fn()
}));

vi.mock('$lib/api/enrichment', () => ({
  setSeriesCvId: vi.fn()
}));

vi.mock('$lib/api/cv', () => ({
  searchVolumes: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

vi.mock('$app/navigation', () => ({
  goto: vi.fn(),
  invalidateAll: vi.fn()
}));

beforeEach(() => {
  vi.clearAllMocks();
  // Default CV search resolves to an empty result. Tests that drive
  // the picker override this with concrete results.
  vi.mocked(searchVolumes).mockResolvedValue({
    results: [],
    filtered_publisher: 0,
    filtered_in_library: 0
  });
});

function cvResult(over: Partial<SeriesSearchResult> = {}): SeriesSearchResult {
  return {
    cv_id: 9999,
    name: 'Picked Volume',
    start_year: 2024,
    publisher: 'Image',
    issue_count: 5,
    cover_url: null,
    description: null,
    ...over
  };
}

function seriesDetail(over: Partial<SeriesDetail> = {}): SeriesDetail {
  // `owned_file_count` is sourced from the server's authoritative
  // count (matches the delete-guard SQL). For test fixtures we
  // derive it from the issues list as a default — overrides take
  // precedence so tests can exercise the "server-says-N-but-issues-
  // show-zero" mismatch directly.
  const issues = over.issues ?? [];
  const derivedOwnedCount = issues.filter((i) => i.file?.status === 'owned').length;
  return {
    id: 1,
    cv_id: 12345,
    metron_id: null,
    title: 'Adventureman',
    sort_title: 'adventureman',
    start_year: 2020,
    publisher: 'Image',
    description: null,
    cover_url: null,
    created_at: '2026-05-20 00:00:00',
    updated_at: '2026-05-20 00:00:00',
    issues,
    owned_file_count: derivedOwnedCount,
    ...over
  };
}

function pageData(series: SeriesDetail, pullEntry: PullEntry | null = null) {
  return { props: { data: { series, pullEntry } } };
}

describe('series detail page', () => {
  it('renders Refresh and the CV-flavored empty-issues hint for a CV-linked series', () => {
    render(Page, pageData(seriesDetail({ cv_id: 12345 })));

    expect(screen.getByRole('button', { name: /Refresh/ })).toBeInTheDocument();
    expect(screen.getByText(/Hit Refresh to fetch from ComicVine/)).toBeInTheDocument();
  });

  it('confirms with the computed folder path and sends deleteFiles=true', async () => {
    vi.mocked(deleteSeries).mockResolvedValue({ deleted: 1 });

    // Two linked files of three issues — the modal should say "2 files".
    const fileSummary = (id: number): SeriesDetail['issues'][0]['file'] => ({
      id,
      path_relative: `Adventureman (2020)/${id}.cbz`,
      status: 'owned',
      is_present: true
    });
    render(
      Page,
      pageData(
        seriesDetail({
          title: 'Adventureman',
          start_year: 2020,
          issues: [
            {
              id: 1,
              number: '1',
              title: null,
              cover_date: null,
              cover_url: null,
              cv_issue_id: null,
              metron_issue_id: null,
              created_at: '2026-05-20 00:00:00',
              updated_at: '2026-05-20 00:00:00',
              file: fileSummary(1)
            },
            {
              id: 2,
              number: '2',
              title: null,
              cover_date: null,
              cover_url: null,
              cv_issue_id: null,
              metron_issue_id: null,
              created_at: '2026-05-20 00:00:00',
              updated_at: '2026-05-20 00:00:00',
              file: fileSummary(2)
            },
            {
              id: 3,
              number: '3',
              title: null,
              cover_date: null,
              cover_url: null,
              cv_issue_id: null,
              metron_issue_id: null,
              created_at: '2026-05-20 00:00:00',
              updated_at: '2026-05-20 00:00:00',
              file: null
            }
          ]
        })
      )
    );

    await fireEvent.click(screen.getByRole('button', { name: /Delete series/ }));

    // Modal copy carries the destructive language and exact folder.
    expect(screen.getByText(/permanently delete the series folder/)).toBeInTheDocument();
    expect(screen.getByText(/2 files from disk/)).toBeInTheDocument();
    expect(screen.getByText('This cannot be undone.')).toBeInTheDocument();
    expect(screen.getByText('Adventureman (2020)/')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Delete series and files' }));

    await waitFor(() => expect(deleteSeries).toHaveBeenCalledWith(1, { deleteFiles: true }));
  });

  it('renders the bare title (no parentheses) when start_year is null', async () => {
    vi.mocked(deleteSeries).mockResolvedValue({ deleted: 1 });
    // The skip-modal-when-zero-files rule means the modal only opens
    // when there's at least one linked file — give the series one
    // owned file so the modal renders and we can assert its copy.
    render(
      Page,
      pageData(
        seriesDetail({
          title: 'Yearless',
          start_year: null,
          issues: [
            {
              id: 1,
              number: '1',
              title: null,
              cover_date: null,
              cover_url: null,
              cv_issue_id: null,
              metron_issue_id: null,
              created_at: '2026-05-20 00:00:00',
              updated_at: '2026-05-20 00:00:00',
              file: {
                id: 1,
                path_relative: 'Yearless/1.cbz',
                status: 'owned',
                is_present: true
              }
            }
          ]
        })
      )
    );

    await fireEvent.click(screen.getByRole('button', { name: /Delete series/ }));
    // Bare title, no `(YYYY)` suffix.
    expect(screen.getByText('Yearless/')).toBeInTheDocument();
    expect(screen.queryByText(/\(\d{4}\)\//)).not.toBeInTheDocument();
  });

  it('skips the modal and calls deleteSeries without deleteFiles when no linked files', async () => {
    // Per the spec's "default behavior should only apply if zero files
    // on disk" rule: the click is a straight DB-only delete.
    vi.mocked(deleteSeries).mockResolvedValue({ deleted: 1 });
    render(Page, pageData(seriesDetail({ title: 'Empty', start_year: 2020, issues: [] })));

    await fireEvent.click(screen.getByRole('button', { name: /Delete series/ }));

    await waitFor(() => expect(deleteSeries).toHaveBeenCalledWith(1, {}));
    // No modal copy.
    expect(screen.queryByText(/Folder to be removed/)).not.toBeInTheDocument();
    expect(screen.queryByText(/permanently delete the series folder/)).not.toBeInTheDocument();
  });

  it('falls back to the issues-array count when owned_file_count is missing from the response', async () => {
    // Safari-specific regression: the field arrived as undefined on
    // the client even though the API payload included a positive
    // value. The defensive derived prefers the server count when
    // present and falls back to the issues array when it isn't.
    // Without the fallback, undefined would slip past the `=== 0`
    // guard, the user would click Delete, runDelete(false) would
    // post to the backend, and the join-based guard there would 409.
    vi.mocked(deleteSeries).mockResolvedValue({ deleted: 1 });
    render(
      Page,
      pageData({
        ...seriesDetail({
          title: 'Field Missing',
          start_year: 2022,
          issues: [
            {
              id: 1,
              series_id: 1,
              cv_issue_id: null,
              metron_issue_id: null,
              number: '1',
              title: null,
              cover_date: null,
              cover_url: null,
              summary: null,
              created_at: '2026-05-20 00:00:00',
              updated_at: '2026-05-20 00:00:00',
              file: {
                id: 1,
                path_relative: 'Field Missing (2022)/01.cbz',
                status: 'owned',
                is_present: true
              }
            }
          ]
        }),
        // Field deliberately missing — simulates the Safari bug
        // shape where the API field came back undefined.
        owned_file_count: undefined as never
      })
    );

    await fireEvent.click(screen.getByRole('button', { name: /Delete series/ }));

    // The modal renders — fallback derived count is 1.
    expect(screen.getByText(/Folder to be removed/)).toBeInTheDocument();
    expect(screen.getByText(/1 file from disk/)).toBeInTheDocument();
    // No direct call to deleteSeries; the user still has to confirm.
    expect(deleteSeries).not.toHaveBeenCalled();
  });

  it('opens the modal on owned_file_count > 0 even when issues[].file is empty', async () => {
    // Bug regression: shallow / unenriched series surface owned files
    // through the join-based server count, but `issues[].file` can
    // come back null on every issue row. The pre-fix guard derived
    // the count from `issues[].file`, read 0, skipped the modal,
    // and ran a delete that 409'd because the backend's join-based
    // owned-files guard saw real files. With the authoritative
    // `owned_file_count` field driving the guard, the modal opens
    // and the user gets a chance to confirm `delete_files=true`.
    vi.mocked(deleteSeries).mockResolvedValue({ deleted: 1 });
    render(
      Page,
      pageData(
        seriesDetail({
          title: 'Unenriched',
          start_year: 2021,
          cv_id: null,
          issues: [
            {
              id: 1,
              series_id: 1,
              cv_issue_id: null,
              metron_issue_id: null,
              number: '1',
              title: null,
              cover_date: null,
              cover_url: null,
              summary: null,
              created_at: '2026-05-20 00:00:00',
              updated_at: '2026-05-20 00:00:00',
              // The bug shape: a real owned file on disk that the
              // per-issue lookup didn't surface back on this response.
              file: null
            }
          ],
          owned_file_count: 3
        })
      )
    );

    await fireEvent.click(screen.getByRole('button', { name: /Delete series/ }));

    // The destructive-confirm modal renders with the server count.
    expect(screen.getByText(/Folder to be removed/)).toBeInTheDocument();
    expect(screen.getByText(/3 files from disk/)).toBeInTheDocument();
    // Critically: deleteSeries has NOT been called yet — the user
    // still needs to click "Delete series and files" inside the
    // modal. Without the fix this test fails because the click
    // would have already triggered runDelete(false) and 409'd.
    expect(deleteSeries).not.toHaveBeenCalled();
  });

  it('hides Refresh and shows the shallow empty-issues hint for a cv_id-NULL series', () => {
    // A.9 shallow-series UX hot-fix: the Refresh button calls a CV
    // endpoint that 400s when cv_id is NULL, and the "Hit Refresh"
    // hint actively directs the user at the broken affordance.
    // Shallow series get neither.
    render(Page, pageData(seriesDetail({ cv_id: null, publisher: null })));

    expect(screen.queryByRole('button', { name: /Refresh/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/Hit Refresh to fetch from ComicVine/)).not.toBeInTheDocument();
    expect(
      screen.getByText(/will appear here as the next scan parses and attaches them/)
    ).toBeInTheDocument();
  });

  // -------- Fix / Change match picker ---------------------------------

  it('shows "Fix match" on a shallow series (cv_id null)', () => {
    render(Page, pageData(seriesDetail({ cv_id: null })));
    expect(screen.getByRole('button', { name: /Fix match/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Change match/ })).not.toBeInTheDocument();
  });

  it('shows "Change match" on a CV-linked series', () => {
    render(Page, pageData(seriesDetail({ cv_id: 12345 })));
    expect(screen.getByRole('button', { name: /Change match/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Fix match/ })).not.toBeInTheDocument();
  });

  it('opens the CV picker and pre-populates with the series title', async () => {
    render(Page, pageData(seriesDetail({ cv_id: null, title: 'Wolverine', start_year: 2024 })));
    await fireEvent.click(screen.getByRole('button', { name: /Fix match/ }));

    // Picker section is now visible.
    expect(screen.getByLabelText('ComicVine match picker')).toBeInTheDocument();
    expect(screen.getByText('Find ComicVine match')).toBeInTheDocument();
    // CvSearchInput auto-runs its initialQuery — the title — through
    // searchVolumes(). Use waitFor since the search is debounced.
    await waitFor(() =>
      expect(searchVolumes).toHaveBeenCalledWith('Wolverine', expect.any(Object))
    );
  });

  it('picking a CV result fires setSeriesCvId and invalidates the page', async () => {
    const picked = cvResult({ cv_id: 4242, name: 'Wolverine (2024)' });
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [picked],
      filtered_publisher: 0,
      filtered_in_library: 0
    });
    vi.mocked(setSeriesCvId).mockResolvedValue({
      ...seriesDetail({ cv_id: picked.cv_id, title: picked.name }),
      issues: undefined as never
    });

    render(Page, pageData(seriesDetail({ id: 7, cv_id: null, title: 'Wolverine' })));
    await fireEvent.click(screen.getByRole('button', { name: /Fix match/ }));

    // Wait for the first result to render, then click it. CvSearchInput
    // renders result rows as buttons titled with the volume name.
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Wolverine \(2024\)/ })).toBeInTheDocument()
    );
    await fireEvent.click(screen.getByRole('button', { name: /Wolverine \(2024\)/ }));

    await waitFor(() => expect(setSeriesCvId).toHaveBeenCalledWith(7, 4242));
    // Page invalidation happens on success — the rebuilt issue list /
    // covers / titles come back through `data`.
    await waitFor(() => expect(invalidateAll).toHaveBeenCalled());
    // Picker closes once the PATCH resolves.
    expect(screen.queryByLabelText('ComicVine match picker')).not.toBeInTheDocument();
  });

  it('keeps the picker open and warns on cv_id_in_use without invalidating', async () => {
    const picked = cvResult({ cv_id: 4242, name: 'Already Linked' });
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [picked],
      filtered_publisher: 0,
      filtered_in_library: 0
    });
    vi.mocked(setSeriesCvId).mockRejectedValue(
      new ApiError(409, 'conflict.cv_id_in_use', 'taken', {
        existing_series_id: 99,
        existing_series_title: 'The Other Wolverine'
      })
    );

    render(Page, pageData(seriesDetail({ id: 7, cv_id: null, title: 'Wolverine' })));
    await fireEvent.click(screen.getByRole('button', { name: /Fix match/ }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Already Linked/ })).toBeInTheDocument()
    );
    await fireEvent.click(screen.getByRole('button', { name: /Already Linked/ }));

    await waitFor(() => expect(setSeriesCvId).toHaveBeenCalled());
    expect(invalidateAll).not.toHaveBeenCalled();
    const { toast } = await import('$lib/stores/toast.svelte');
    expect(toast.warning).toHaveBeenCalledWith(
      'That ComicVine volume is already linked to The Other Wolverine.'
    );
    // Picker stays open so the user can pick a different volume.
    expect(screen.getByLabelText('ComicVine match picker')).toBeInTheDocument();
  });

  // -------- Copy Path --------------------------------------------------
  //
  // The old "Show in Finder" affordance was structurally broken: LongBox
  // runs in Docker, so a backend `open -R` can't reach the host shell
  // and a frontend `file://` URL is blocked by browsers from an
  // `http://` origin. Replaced with a clipboard copy — the user pastes
  // into Finder's Cmd+Shift+G to actually open the folder.

  it('Copy Path writes the host-translated path to the clipboard and toasts success', async () => {
    const { getSeriesFolderPath } = await import('$lib/api/series');
    vi.mocked(getSeriesFolderPath).mockResolvedValue({
      container_path: '/library/Adventureman (2020)',
      host_path: '/Volumes/Comics/Adventureman (2020)',
      host_path_configured: true
    });
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(Page, pageData(seriesDetail({ id: 7 })));

    await fireEvent.click(screen.getByRole('button', { name: /Copy Path/ }));

    await waitFor(() => expect(getSeriesFolderPath).toHaveBeenCalledWith(7));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith('/Volumes/Comics/Adventureman (2020)')
    );
    const { toast } = await import('$lib/stores/toast.svelte');
    expect(toast.success).toHaveBeenCalledWith(expect.stringContaining('Cmd+Shift+G'));
  });

  it('Copy Path copies the container path and warns when host_library_path is unset', async () => {
    const { getSeriesFolderPath } = await import('$lib/api/series');
    vi.mocked(getSeriesFolderPath).mockResolvedValue({
      container_path: '/library/Adventureman (2020)',
      host_path: '/library/Adventureman (2020)',
      host_path_configured: false
    });
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(Page, pageData(seriesDetail({ id: 7 })));

    await fireEvent.click(screen.getByRole('button', { name: /Copy Path/ }));

    await waitFor(() => expect(writeText).toHaveBeenCalledWith('/library/Adventureman (2020)'));
    const { toast } = await import('$lib/stores/toast.svelte');
    expect(toast.warning).toHaveBeenCalledWith(expect.stringContaining('HOST_LIBRARY_PATH'));
  });

  it('Copy Path reveals a fallback input when the Clipboard API rejects', async () => {
    const { getSeriesFolderPath } = await import('$lib/api/series');
    vi.mocked(getSeriesFolderPath).mockResolvedValue({
      container_path: '/library/Adventureman (2020)',
      host_path: '/Volumes/Comics/Adventureman (2020)',
      host_path_configured: true
    });
    // Simulate `navigator.clipboard.writeText` being denied (Safari
    // over LAN, insecure context, headless without a clipboard
    // backend). The fallback input must surface so the user can
    // finish the copy with a manual Cmd+C.
    const writeText = vi.fn().mockRejectedValue(new Error('NotAllowedError'));
    Object.assign(navigator, { clipboard: { writeText } });
    render(Page, pageData(seriesDetail({ id: 7 })));

    await fireEvent.click(screen.getByRole('button', { name: /Copy Path/ }));

    await waitFor(() => {
      const input = screen.getByLabelText('Series folder path') as HTMLInputElement;
      expect(input.value).toBe('/Volumes/Comics/Adventureman (2020)');
      expect(input.readOnly).toBe(true);
    });
    const { toast } = await import('$lib/stores/toast.svelte');
    expect(toast.warning).toHaveBeenCalledWith(
      expect.stringContaining('Could not copy automatically')
    );
  });
});
