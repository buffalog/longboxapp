// Component test for the Library Tidy page. The route +page.svelte is an
// ordinary component taking a `data` prop, so it renders in isolation
// with @testing-library/svelte. Beyond exercising one mutation per
// action, this locks the transition/steady-state $derived split: a
// transition phantom must render in exactly one subsection.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  addFolders,
  bulkDeletePhantoms,
  convertFolders,
  deletePhantom,
  dismissFolders,
  keepPhantom,
  type DiscoveredFolder,
  type PhantomSeries
} from '$lib/api/reconcile';
import { searchVolumes } from '$lib/api/cv';
import { ApiError } from '$lib/api/client';
import {
  setSeriesCvId,
  type EnrichmentQueueRow,
  type EnrichmentSummary
} from '$lib/api/enrichment';
import { deleteSeries } from '$lib/api/series';
import { toast } from '$lib/stores/toast.svelte';
import TidyPage from './+page.svelte';

vi.mock('$lib/api/reconcile', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/reconcile')>()),
  addFolders: vi.fn(),
  bulkDeletePhantoms: vi.fn(),
  convertFolders: vi.fn(),
  deletePhantom: vi.fn(),
  dismissFolders: vi.fn(),
  keepPhantom: vi.fn()
}));

vi.mock('$lib/api/cv', () => ({
  searchVolumes: vi.fn()
}));

vi.mock('$lib/api/enrichment', () => ({
  setSeriesCvId: vi.fn(),
  getEnrichmentSummary: vi.fn(),
  getEnrichmentQueue: vi.fn()
}));

vi.mock('$lib/api/series', () => ({
  deleteSeries: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

function phantom(over: Partial<PhantomSeries> = {}): PhantomSeries {
  return {
    id: 1,
    title: 'Steady Series',
    sort_title: 'steady series',
    start_year: null,
    publisher: 'Image',
    cover_url: null,
    last_matched_count: 0,
    awaiting_first_download: false,
    auto_tidy_due_at: null,
    ...over
  };
}

function folder(over: Partial<DiscoveredFolder> = {}): DiscoveredFolder {
  return {
    id: 1,
    folder_name: 'Saga (2012)',
    first_seen_at: '2026-05-01T00:00:00',
    last_seen_at: '2026-05-20T00:00:00',
    dismissed_at: null,
    auto_dismissed_at: null,
    file_count: 3,
    ...over
  };
}

function emptyEnrichmentSummary(): EnrichmentSummary {
  return {
    shallow_total: 0,
    awaiting_attempt: 0,
    cooldown_waiting: 0,
    is_running: false,
    recent_outcomes: {
      matched: 0,
      partial_merge: 0,
      no_results: 0,
      low_confidence: 0,
      multi_match: 0,
      year_mismatch: 0,
      count_mismatch: 0,
      collision_disabled: 0,
      cv_id_collision: 0,
      set_cv_id_race_lost: 0,
      error: 0
    }
  };
}

function pageData(
  over: {
    phantoms?: PhantomSeries[];
    untracked?: DiscoveredFolder[];
    enrichmentSummary?: EnrichmentSummary;
    enrichmentQueue?: EnrichmentQueueRow[];
  } = {}
) {
  const phantoms = over.phantoms ?? [];
  return {
    props: {
      data: {
        // `libraryRoot` rides in from a parent layout load; the tidy
        // page never reads it, but the route's `data` type requires it.
        libraryRoot: null,
        phantoms: {
          all_zero_owned: phantoms,
          with_transition: phantoms.filter((p) => p.last_matched_count > 0)
        },
        untracked: over.untracked ?? [],
        enrichmentSummary: over.enrichmentSummary ?? emptyEnrichmentSummary(),
        enrichmentQueue: over.enrichmentQueue ?? []
      }
    }
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('library tidy page', () => {
  it('renders a transition phantom only under "Recently lost files"', () => {
    render(
      TidyPage,
      pageData({
        phantoms: [
          phantom({ id: 1, title: 'Transition Series', last_matched_count: 5 }),
          phantom({ id: 2, title: 'Steady Series', last_matched_count: 0 })
        ]
      })
    );
    expect(screen.getByText('Recently lost files')).toBeInTheDocument();
    expect(screen.getByText('Empty series')).toBeInTheDocument();
    // Disjoint partition: the transition phantom renders exactly once
    // ("Recently lost files"), never duplicated into "Empty series".
    expect(screen.getAllByText('Transition Series')).toHaveLength(1);
    expect(screen.getByText('Steady Series')).toBeInTheDocument();
    expect(screen.getByText(/Had 5 matched files/)).toBeInTheDocument();
  });

  it('shows the tidy empty state when nothing needs reconciling', () => {
    render(TidyPage, pageData());
    expect(screen.getByText('Your library is tidy')).toBeInTheDocument();
  });

  it('removes a phantom when Remove is clicked', async () => {
    vi.mocked(deletePhantom).mockResolvedValue({ deleted: 2 });
    render(TidyPage, pageData({ phantoms: [phantom({ id: 2, title: 'Steady Series' })] }));

    await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => expect(deletePhantom).toHaveBeenCalledWith(2));
    await waitFor(() => expect(screen.queryByText('Steady Series')).not.toBeInTheDocument());
  });

  it('bulk-removes selected phantoms', async () => {
    vi.mocked(bulkDeletePhantoms).mockResolvedValue({ deleted: [2, 3], skipped: [] });
    render(
      TidyPage,
      pageData({
        phantoms: [
          phantom({ id: 2, title: 'Steady Two' }),
          phantom({ id: 3, title: 'Steady Three' })
        ]
      })
    );

    await fireEvent.click(screen.getByLabelText('Select all empty series'));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove selected' }));

    await waitFor(() => expect(bulkDeletePhantoms).toHaveBeenCalledWith([2, 3]));
    await waitFor(() => {
      expect(screen.queryByText('Steady Two')).not.toBeInTheDocument();
      expect(screen.queryByText('Steady Three')).not.toBeInTheDocument();
    });
  });

  it('keeps a transition phantom, moving it to Empty series', async () => {
    vi.mocked(keepPhantom).mockResolvedValue({ kept: 1 });
    render(
      TidyPage,
      pageData({ phantoms: [phantom({ id: 1, title: 'Lost Series', last_matched_count: 4 })] })
    );
    expect(screen.getByText('Recently lost files')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Keep' }));

    await waitFor(() => expect(keepPhantom).toHaveBeenCalledWith(1));
    // last_matched_count -> 0: the transition subsection empties and the
    // row drops to "Empty series".
    await waitFor(() =>
      expect(screen.queryByText('Recently lost files')).not.toBeInTheDocument()
    );
    expect(screen.getByText('Lost Series')).toBeInTheDocument();
  });

  it('renders a scheduled-for-removal phantom only under "Scheduled for automatic removal"', () => {
    render(
      TidyPage,
      pageData({
        phantoms: [
          phantom({ id: 1, title: 'Doomed Series', auto_tidy_due_at: '2026-06-05 03:00:00' }),
          phantom({ id: 2, title: 'Steady Series' })
        ]
      })
    );
    expect(screen.getByText('Scheduled for automatic removal')).toBeInTheDocument();
    // The scheduled bucket outranks every other — the row appears once.
    expect(screen.getAllByText('Doomed Series')).toHaveLength(1);
    expect(screen.getByText(/Will be removed on/)).toBeInTheDocument();
  });

  it('renders an awaiting-first-download phantom in its own subsection', () => {
    render(
      TidyPage,
      pageData({
        phantoms: [phantom({ id: 1, title: 'Subscribed Series', awaiting_first_download: true })]
      })
    );
    expect(screen.getByText('Awaiting first download')).toBeInTheDocument();
    expect(screen.getByText('Subscribed Series')).toBeInTheDocument();
    // It is expected state, not a problem — no "Empty series" bulk surface.
    expect(screen.queryByText('Empty series')).not.toBeInTheDocument();
  });

  it('cancels a scheduled removal when Keep is clicked', async () => {
    vi.mocked(keepPhantom).mockResolvedValue({ kept: 1 });
    render(
      TidyPage,
      pageData({
        phantoms: [
          phantom({ id: 1, title: 'Doomed Series', auto_tidy_due_at: '2026-06-05 03:00:00' })
        ]
      })
    );
    expect(screen.getByText('Scheduled for automatic removal')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Keep' }));

    await waitFor(() => expect(keepPhantom).toHaveBeenCalledWith(1));
    // auto_tidy_due_at -> null: the scheduled subsection empties and the
    // row drops to "Empty series".
    await waitFor(() =>
      expect(screen.queryByText('Scheduled for automatic removal')).not.toBeInTheDocument()
    );
    expect(screen.getByText('Doomed Series')).toBeInTheDocument();
  });

  it('dismisses an untracked folder', async () => {
    vi.mocked(dismissFolders).mockResolvedValue({ dismissed: 1 });
    render(TidyPage, pageData({ untracked: [folder({ folder_name: 'Saga (2012)' })] }));

    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));

    await waitFor(() => expect(dismissFolders).toHaveBeenCalledWith(['Saga (2012)']));
    await waitFor(() => expect(screen.queryByText('Saga (2012)')).not.toBeInTheDocument());
  });

  it('bulk-converts selected untracked folders to tracked series', async () => {
    vi.mocked(convertFolders).mockResolvedValue({
      results: [
        { folder_name: 'Wytches (2014)', status: 'added', series_id: 10 },
        { folder_name: 'Saga (2012)', status: 'added', series_id: 11 }
      ]
    });
    render(
      TidyPage,
      pageData({
        untracked: [
          folder({ folder_name: 'Wytches (2014)' }),
          folder({ folder_name: 'Saga (2012)' })
        ]
      })
    );

    await fireEvent.click(screen.getByLabelText('Select all untracked folders'));
    await fireEvent.click(screen.getByRole('button', { name: 'Convert 2 selected' }));

    await waitFor(() =>
      expect(convertFolders).toHaveBeenCalledWith(['Wytches (2014)', 'Saga (2012)'])
    );
    // Converted folders drop out of the untracked list.
    await waitFor(() => {
      expect(screen.queryByText('Wytches (2014)')).not.toBeInTheDocument();
      expect(screen.queryByText('Saga (2012)')).not.toBeInTheDocument();
    });
  });

  it('counts linked results separately in the toast and drops them from untracked', async () => {
    // A.9 hot-fix: a folder whose (title, year) matches an existing
    // series surfaces as `linked`, not `added`. The toast splits the
    // two counts; both kinds drop from the untracked list.
    vi.mocked(convertFolders).mockResolvedValue({
      results: [
        { folder_name: 'Wytches (2014)', status: 'added', series_id: 10 },
        { folder_name: 'Saga (2012)', status: 'linked', series_id: 7 }
      ]
    });
    render(
      TidyPage,
      pageData({
        untracked: [
          folder({ folder_name: 'Wytches (2014)' }),
          folder({ folder_name: 'Saga (2012)' })
        ]
      })
    );
    await fireEvent.click(screen.getByLabelText('Select all untracked folders'));
    await fireEvent.click(screen.getByRole('button', { name: 'Convert 2 selected' }));

    await waitFor(() => {
      expect(screen.queryByText('Wytches (2014)')).not.toBeInTheDocument();
      expect(screen.queryByText('Saga (2012)')).not.toBeInTheDocument();
    });
    expect(toast.success).toHaveBeenCalledWith(expect.stringContaining('1 added'));
    expect(toast.success).toHaveBeenCalledWith(expect.stringContaining('1 linked'));
  });

  it('adds an untracked folder via the ComicVine modal', async () => {
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [
        {
          cv_id: 18000,
          name: 'Saga',
          start_year: 2012,
          publisher: 'Image',
          issue_count: 60,
          cover_url: null,
          description: null
        }
      ],
      filtered_count: 0
    });
    vi.mocked(addFolders).mockResolvedValue({
      succeeded: [{ folder_name: 'Saga (2012)', series_id: 99 }],
      failed: []
    });
    render(TidyPage, pageData({ untracked: [folder({ folder_name: 'Saga (2012)' })] }));

    await fireEvent.click(screen.getByRole('button', { name: 'Add to LongBox' }));
    // CvSearchInput runs an initial search seeded from the folder-name
    // hint; wait out its debounce for the result button.
    const result = await screen.findByRole('button', { name: /Saga/ });
    await fireEvent.click(result);
    // Selecting a volume reveals the modal's "Add" confirm button.
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() =>
      expect(addFolders).toHaveBeenCalledWith([{ folder_name: 'Saga (2012)', cv_id: 18000 }])
    );
    await waitFor(() => expect(screen.queryByText('Saga (2012)')).not.toBeInTheDocument());
  });

  it('surfaces an inline error in the add modal when the add fails', async () => {
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [
        {
          cv_id: 18000,
          name: 'Saga',
          start_year: 2012,
          publisher: 'Image',
          issue_count: 60,
          cover_url: null,
          description: null
        }
      ],
      filtered_count: 0
    });
    // POST /reconcile/add resolves 200 with a populated `failed` array.
    vi.mocked(addFolders).mockResolvedValue({
      succeeded: [],
      failed: [{ folder_name: 'Saga (2012)', error: 'ComicVine rate limited' }]
    });
    render(TidyPage, pageData({ untracked: [folder({ folder_name: 'Saga (2012)' })] }));

    await fireEvent.click(screen.getByRole('button', { name: 'Add to LongBox' }));
    const result = await screen.findByRole('button', { name: /Saga/ });
    await fireEvent.click(result);
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    // The failure surfaces inline and the modal stays open for a retry.
    await waitFor(() =>
      expect(screen.getByText('ComicVine rate limited')).toBeInTheDocument()
    );
    expect(screen.getByRole('button', { name: 'Add' })).toBeInTheDocument();
  });

  it('surfaces a delete failure in the page error banner', async () => {
    vi.mocked(deletePhantom).mockRejectedValue(
      new ApiError(409, 'conflict.series_has_owned_files', 'Files reappeared on disk')
    );
    render(TidyPage, pageData({ phantoms: [phantom({ id: 2, title: 'Stubborn' })] }));

    await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() =>
      expect(screen.getByText(/Files reappeared on disk/)).toBeInTheDocument()
    );
    // The delete failed — the row is still present.
    expect(screen.getByText('Stubborn')).toBeInTheDocument();
  });

  // -------- enrichment review queue ----------------------------------

  function enrichmentRow(over: Partial<EnrichmentQueueRow> = {}): EnrichmentQueueRow {
    return {
      id: 42,
      title: 'Ambiguous Title',
      start_year: 2024,
      last_enrichment_outcome: 'multi_match',
      last_enrichment_error: null,
      owned_count: 6,
      ...over
    };
  }

  it('hides the enrichment-review section when the queue is empty', () => {
    render(TidyPage, pageData({ phantoms: [phantom()] }));
    expect(screen.queryByText('Enrichment needs review')).not.toBeInTheDocument();
  });

  it('renders the enrichment-review section with a per-row badge + owned count', () => {
    render(
      TidyPage,
      pageData({
        enrichmentQueue: [
          enrichmentRow({ id: 42, title: 'Ambiguous', last_enrichment_outcome: 'multi_match' }),
          enrichmentRow({ id: 99, title: 'Year Off', last_enrichment_outcome: 'year_mismatch' })
        ]
      })
    );
    expect(screen.getByText('Enrichment needs review')).toBeInTheDocument();
    expect(screen.getByText(/2\s+series need.*ComicVine pick/)).toBeInTheDocument();
    expect(screen.getByText('Ambiguous')).toBeInTheDocument();
    expect(screen.getByText('Year Off')).toBeInTheDocument();
    expect(screen.getByText('Multiple matches')).toBeInTheDocument();
    expect(screen.getByText('Year mismatch')).toBeInTheDocument();
  });

  it('PATCHes cv-id and optimistically removes a row when CvSearchInput onSelect fires', async () => {
    vi.mocked(setSeriesCvId).mockResolvedValue({
      id: 42,
      cv_id: 12345,
      metron_id: null,
      title: 'Resolved',
      sort_title: 'resolved',
      start_year: 2024,
      publisher: 'Marvel',
      description: null,
      cover_url: null,
      created_at: '2026-01-01T00:00:00',
      updated_at: '2026-06-02T00:00:00'
    });
    vi.mocked(searchVolumes).mockResolvedValue({ results: [], filtered_count: 0 });
    render(
      TidyPage,
      pageData({ enrichmentQueue: [enrichmentRow({ id: 42, title: 'Ambiguous' })] })
    );

    // Type into the CvSearchInput, then directly trigger the search +
    // simulate the onSelect call by mounting the CV search response.
    // Easier: bypass the input entirely and call the handler via the
    // page's exposed surface. CvSearchInput's onSelect is the only
    // path to the handler, so simulate it by mocking searchVolumes to
    // return a single result, then click it.
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [
        {
          cv_id: 12345,
          name: 'Real Volume',
          start_year: 2024,
          publisher: 'Marvel',
          issue_count: 6,
          cover_url: null,
          description: null
        }
      ],
      filtered_count: 0
    });
    // Find the CvSearchInput's input field (search role) and type.
    const input = screen.getByPlaceholderText('Series name…') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Real Volume' } });
    // The debounce is 300ms — wait for the result button to appear.
    const pickButton = await waitFor(() =>
      screen.getByRole('button', { name: /Real Volume/ })
    );
    await fireEvent.click(pickButton);

    await waitFor(() => expect(setSeriesCvId).toHaveBeenCalledWith(42, 12345));
    // Optimistic removal — the row is gone, the section hides because
    // the queue is now empty.
    await waitFor(() =>
      expect(screen.queryByText('Enrichment needs review')).not.toBeInTheDocument()
    );
    expect(toast.success).toHaveBeenCalledWith(expect.stringContaining('Ambiguous'));
  });

  it('renders an inline collision warning with a force-delete button when owned_count > 0', async () => {
    vi.mocked(setSeriesCvId).mockRejectedValue(
      new ApiError(
        409,
        'conflict.cv_id_in_use',
        'This ComicVine volume is already linked to "Other Series" (series #7).',
        { existing_series_id: 7, existing_series_title: 'Other Series' }
      )
    );
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [
        {
          cv_id: 12345,
          name: 'Real Volume',
          start_year: 2024,
          publisher: 'Marvel',
          issue_count: 6,
          cover_url: null,
          description: null
        }
      ],
      filtered_count: 0
    });
    // owned_count: 6 (the default) — the inline warning explains the
    // files are almost certainly misassigned, and offers a force-delete
    // affordance ("Delete duplicate anyway").
    render(
      TidyPage,
      pageData({ enrichmentQueue: [enrichmentRow({ id: 42, title: 'Ambiguous' })] })
    );

    const input = screen.getByPlaceholderText('Series name…') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Real' } });
    const pickButton = await waitFor(() =>
      screen.getByRole('button', { name: /Real Volume/ })
    );
    await fireEvent.click(pickButton);

    await waitFor(() =>
      expect(screen.getByText(/Already linked to "Other Series"/)).toBeInTheDocument()
    );
    expect(screen.getByText(/almost certainly misassigned/)).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /Delete duplicate anyway/ })
    ).toBeInTheDocument();
    // Plain "Delete duplicate" (no "anyway") is reserved for the
    // zero-owned-files case.
    expect(
      screen.queryByRole('button', { name: /^Delete duplicate$/ })
    ).not.toBeInTheDocument();
    expect(screen.getByText('Ambiguous')).toBeInTheDocument();
    expect(toast.warning).not.toHaveBeenCalled();
  });

  it('force-deletes the duplicate when "Delete duplicate anyway" is clicked', async () => {
    vi.mocked(setSeriesCvId).mockRejectedValue(
      new ApiError(
        409,
        'conflict.cv_id_in_use',
        'This ComicVine volume is already linked to "Real Series" (series #7).',
        { existing_series_id: 7, existing_series_title: 'Real Series' }
      )
    );
    vi.mocked(deleteSeries).mockResolvedValue({ deleted: 42 });
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [
        {
          cv_id: 12345,
          name: 'Real Volume',
          start_year: 2024,
          publisher: 'Marvel',
          issue_count: 6,
          cover_url: null,
          description: null
        }
      ],
      filtered_count: 0
    });
    render(
      TidyPage,
      pageData({
        enrichmentQueue: [enrichmentRow({ id: 42, title: 'Misassigned Dup', owned_count: 6 })]
      })
    );

    const input = screen.getByPlaceholderText('Series name…') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Real' } });
    const pickButton = await waitFor(() =>
      screen.getByRole('button', { name: /Real Volume/ })
    );
    await fireEvent.click(pickButton);

    const forceBtn = await waitFor(() =>
      screen.getByRole('button', { name: /Delete duplicate anyway/ })
    );
    await fireEvent.click(forceBtn);

    await waitFor(() =>
      expect(deleteSeries).toHaveBeenCalledWith(42, { force: true })
    );
    await waitFor(() =>
      expect(screen.queryByText('Enrichment needs review')).not.toBeInTheDocument()
    );
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringContaining('queued for re-match')
    );
  });

  it('shows a Delete duplicate button when the colliding row has zero owned files', async () => {
    vi.mocked(setSeriesCvId).mockRejectedValue(
      new ApiError(
        409,
        'conflict.cv_id_in_use',
        'This ComicVine volume is already linked to "Real Series" (series #7).',
        { existing_series_id: 7, existing_series_title: 'Real Series' }
      )
    );
    vi.mocked(deleteSeries).mockResolvedValue({ deleted: 42 });
    vi.mocked(searchVolumes).mockResolvedValue({
      results: [
        {
          cv_id: 12345,
          name: 'Real Volume',
          start_year: 2024,
          publisher: 'Marvel',
          issue_count: 6,
          cover_url: null,
          description: null
        }
      ],
      filtered_count: 0
    });
    render(
      TidyPage,
      pageData({
        enrichmentQueue: [enrichmentRow({ id: 42, title: 'Duplicate Title', owned_count: 0 })]
      })
    );

    const input = screen.getByPlaceholderText('Series name…') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Real' } });
    const pickButton = await waitFor(() =>
      screen.getByRole('button', { name: /Real Volume/ })
    );
    await fireEvent.click(pickButton);

    // Zero-owned case: button label is "Delete duplicate" (no "anyway").
    const deleteBtn = await waitFor(() =>
      screen.getByRole('button', { name: /^Delete duplicate$/ })
    );
    expect(screen.getByText(/Already linked to "Real Series"/)).toBeInTheDocument();
    await fireEvent.click(deleteBtn);

    await waitFor(() =>
      expect(deleteSeries).toHaveBeenCalledWith(42, { force: false })
    );
    await waitFor(() =>
      expect(screen.queryByText('Enrichment needs review')).not.toBeInTheDocument()
    );
    expect(toast.success).toHaveBeenCalledWith(expect.stringContaining('Duplicate Title'));
  });

  it('keeps skipped rows and warns after a partial bulk delete', async () => {
    vi.mocked(bulkDeletePhantoms).mockResolvedValue({
      deleted: [2],
      skipped: [{ series_id: 3, reason: 'owned files reappeared' }]
    });
    render(
      TidyPage,
      pageData({
        phantoms: [phantom({ id: 2, title: 'Gone Two' }), phantom({ id: 3, title: 'Kept Three' })]
      })
    );

    await fireEvent.click(screen.getByLabelText('Select all empty series'));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove selected' }));

    await waitFor(() => expect(screen.queryByText('Gone Two')).not.toBeInTheDocument());
    // The skipped row stays; the warning toast names the skipped count.
    expect(screen.getByText('Kept Three')).toBeInTheDocument();
    expect(toast.warning).toHaveBeenCalledWith(expect.stringContaining('skipped 1'));
  });
});
