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
  deletePhantom,
  dismissFolders,
  keepPhantom,
  type DiscoveredFolder,
  type PhantomSeries
} from '$lib/api/reconcile';
import { searchVolumes } from '$lib/api/cv';
import TidyPage from './+page.svelte';

vi.mock('$lib/api/reconcile', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/reconcile')>()),
  addFolders: vi.fn(),
  bulkDeletePhantoms: vi.fn(),
  deletePhantom: vi.fn(),
  dismissFolders: vi.fn(),
  keepPhantom: vi.fn()
}));

vi.mock('$lib/api/cv', () => ({
  searchVolumes: vi.fn()
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
    file_count: 3,
    ...over
  };
}

function pageData(over: { phantoms?: PhantomSeries[]; untracked?: DiscoveredFolder[] } = {}) {
  const phantoms = over.phantoms ?? [];
  return {
    props: {
      data: {
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
    expect(screen.getByText('Zero ownership')).toBeInTheDocument();
    // Disjoint partition: the transition phantom renders exactly once
    // (subsection 1), never duplicated into "Zero ownership".
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

    await fireEvent.click(screen.getByLabelText('Select all zero-ownership series'));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove selected' }));

    await waitFor(() => expect(bulkDeletePhantoms).toHaveBeenCalledWith([2, 3]));
    await waitFor(() => {
      expect(screen.queryByText('Steady Two')).not.toBeInTheDocument();
      expect(screen.queryByText('Steady Three')).not.toBeInTheDocument();
    });
  });

  it('keeps a transition phantom, moving it to Zero ownership', async () => {
    vi.mocked(keepPhantom).mockResolvedValue({ kept: 1 });
    render(
      TidyPage,
      pageData({ phantoms: [phantom({ id: 1, title: 'Lost Series', last_matched_count: 4 })] })
    );
    expect(screen.getByText('Recently lost files')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Keep' }));

    await waitFor(() => expect(keepPhantom).toHaveBeenCalledWith(1));
    // last_matched_count -> 0: the transition subsection empties and the
    // row drops to "Zero ownership".
    await waitFor(() =>
      expect(screen.queryByText('Recently lost files')).not.toBeInTheDocument()
    );
    expect(screen.getByText('Lost Series')).toBeInTheDocument();
  });

  it('dismisses an untracked folder', async () => {
    vi.mocked(dismissFolders).mockResolvedValue({ dismissed: 1 });
    render(TidyPage, pageData({ untracked: [folder({ folder_name: 'Saga (2012)' })] }));

    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));

    await waitFor(() => expect(dismissFolders).toHaveBeenCalledWith(['Saga (2012)']));
    await waitFor(() => expect(screen.queryByText('Saga (2012)')).not.toBeInTheDocument());
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
});
