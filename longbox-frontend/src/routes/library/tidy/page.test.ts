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

function pageData(over: { phantoms?: PhantomSeries[]; untracked?: DiscoveredFolder[] } = {}) {
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
        untracked: over.untracked ?? []
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
