// Regression tests for the /missing page's series grouping. The
// original consecutive-merge loop crashed Svelte's keyed each block
// with each_key_duplicate when the API response interleaved rows from
// two distinct series ids that shared a title (e.g. multiple "The
// Department of Truth" volumes). The Map-based grouper handles any
// sort order.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MissingIssue, MissingResponse } from '$lib/api/missing';
import { searchAllMissing } from '$lib/api/missing';
import { searchSeriesNow } from '$lib/api/pull';
import MissingPage from './+page.svelte';

// goto is called only when a sort/filter control changes; not exercised
// here but the import is reachable, so stub it for safety.
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

vi.mock('$lib/api/missing', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/missing')>()),
  searchAllMissing: vi.fn()
}));

vi.mock('$lib/api/pull', () => ({
  searchSeriesNow: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

// Helper: build a far-future date string so `isSolicited` matches today
// or later regardless of when the test runs.
const FUTURE = '2999-01-01';
const PAST = '2010-05-01';

function missingIssue(over: Partial<MissingIssue> & {
  series_id: number;
  series_title?: string;
}): MissingIssue {
  return {
    issue_id: over.issue_id ?? 0,
    number: over.number ?? '1',
    title: over.title ?? null,
    cover_url: over.cover_url ?? null,
    cover_date: over.cover_date ?? null,
    issue_created_at: '2026-01-01T00:00:00',
    series: {
      id: over.series_id,
      title: over.series_title ?? 'Untitled',
      sort_title: (over.series_title ?? 'untitled').toLowerCase(),
      start_year: null
    }
  };
}

function pageData(missing: MissingIssue[]): {
  props: {
    data: {
      missing: MissingResponse;
      allSeries: never[];
      sort: 'series' | 'cover_date';
      seriesIdFilter: number | null;
    };
  };
} {
  return {
    props: {
      data: {
        missing: { missing, total: missing.length },
        allSeries: [],
        sort: 'series',
        seriesIdFilter: null
      }
    }
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('missing page series grouping', () => {
  it('renders one group per distinct series.id when the API interleaves rows', () => {
    // Two distinct series, same title — interleaved in the API
    // response. The original consecutive-merge logic would have
    // produced [A, B, A] (three groups, two with series_id=668),
    // crashing the keyed `{#each}`. The Map grouper produces [A, B].
    render(
      MissingPage,
      pageData([
        missingIssue({
          issue_id: 1,
          number: '1',
          series_id: 668,
          series_title: 'The Department of Truth'
        }),
        missingIssue({
          issue_id: 2,
          number: '2',
          series_id: 819,
          series_title: 'The Department of Truth'
        }),
        missingIssue({
          issue_id: 3,
          number: '3',
          series_id: 668,
          series_title: 'The Department of Truth'
        })
      ])
    );

    // Two section headers, one per series_id, with correct counts.
    const headers = screen.getAllByRole('heading', { level: 2 });
    expect(headers).toHaveLength(2);
    // Series 668 got two rows, series 819 got one. The count text is
    // rendered next to a leading bullet (`· 2 missing`), so match on a
    // regex rather than the bare phrase.
    expect(screen.getByText(/2 missing/)).toBeInTheDocument();
    expect(screen.getByText(/1 missing/)).toBeInTheDocument();
  });

  it('opens the Interactive Search modal from a missing-issue row', async () => {
    render(
      MissingPage,
      pageData([
        missingIssue({ issue_id: 7, number: '12', series_id: 1, series_title: 'Saga' })
      ])
    );
    await fireEvent.click(
      screen.getByRole('button', { name: /Interactive search for Saga #12/i })
    );
    await waitFor(() =>
      expect(screen.getByText(/Interactive Search — Saga #12/)).toBeInTheDocument()
    );
  });

  it('partitions solicited rows into a separate section and labels the global Search by actionable count', () => {
    render(
      MissingPage,
      pageData([
        // 2 actionable (past or null cover_date)
        missingIssue({
          issue_id: 1,
          number: '1',
          cover_date: PAST,
          series_id: 100,
          series_title: 'Saga'
        }),
        missingIssue({ issue_id: 2, number: '2', series_id: 100, series_title: 'Saga' }),
        // 1 solicited
        missingIssue({
          issue_id: 999,
          number: '99',
          cover_date: FUTURE,
          series_id: 100,
          series_title: 'Saga'
        })
      ])
    );

    // Global search button label uses the actionable count, NOT the total.
    expect(
      screen.getByRole('button', { name: /Search 2 missing/ })
    ).toBeInTheDocument();
    // Header carries both the gross total and the solicited subnote.
    expect(screen.getByText(/3 missing issues/)).toBeInTheDocument();
    expect(screen.getByText(/\(1 solicited\)/)).toBeInTheDocument();

    // The actionable section's per-series count is 2 (solicited issue
    // is filtered out before grouping). The "Search 2 missing" button
    // also contains "2 missing", so disambiguate by leading bullet.
    expect(screen.getByText(/· 2 missing/)).toBeInTheDocument();
    // The solicited section heading exists and has its own per-series
    // count.
    expect(screen.getByRole('heading', { name: /Solicited$/ })).toBeInTheDocument();
    expect(screen.getByText(/· 1 solicited/)).toBeInTheDocument();
  });

  it('hides the global Search button when there are no actionable issues', () => {
    render(
      MissingPage,
      pageData([
        missingIssue({
          issue_id: 1,
          number: '1',
          cover_date: FUTURE,
          series_id: 100,
          series_title: 'Pre-Order Only'
        })
      ])
    );
    expect(
      screen.queryByRole('button', { name: /Search \d+ missing/ })
    ).not.toBeInTheDocument();
    // The solicited section is the only group on the page.
    expect(screen.getByRole('heading', { name: /Solicited$/ })).toBeInTheDocument();
  });

  it('dispatches the global Search-all and reports the summary in a toast', async () => {
    vi.mocked(searchAllMissing).mockResolvedValue({ searched: 5, skipped_solicited: 2 });
    render(
      MissingPage,
      pageData([
        missingIssue({ issue_id: 1, number: '1', cover_date: PAST, series_id: 100, series_title: 'Saga' })
      ])
    );

    await fireEvent.click(screen.getByRole('button', { name: /Search 1 missing/ }));
    await waitFor(() => expect(searchAllMissing).toHaveBeenCalledOnce());

    const { toast } = await import('$lib/stores/toast.svelte');
    expect(toast.success).toHaveBeenCalledWith(
      'Search dispatched for 5 missing issues (2 solicited skipped).'
    );
  });

  it('per-series Search button fires searchSeriesNow for that series only', async () => {
    vi.mocked(searchSeriesNow).mockResolvedValue({ queued: 3, note: null });
    render(
      MissingPage,
      pageData([
        missingIssue({ issue_id: 1, number: '1', cover_date: PAST, series_id: 100, series_title: 'Saga' }),
        missingIssue({ issue_id: 2, number: '5', cover_date: PAST, series_id: 200, series_title: 'Chew' })
      ])
    );

    const buttons = screen.getAllByRole('button', { name: 'Search' });
    expect(buttons).toHaveLength(2);
    await fireEvent.click(buttons[0]!);

    await waitFor(() => expect(searchSeriesNow).toHaveBeenCalledOnce());
    // The first group is Saga (series 100) — that's the id the click
    // routed.
    expect(searchSeriesNow).toHaveBeenCalledWith(100);
  });

  it('bucket-merges all rows for a series even when they are non-contiguous', () => {
    render(
      MissingPage,
      pageData([
        missingIssue({ issue_id: 11, number: '1', series_id: 100, series_title: 'Saga' }),
        missingIssue({ issue_id: 21, number: '5', series_id: 200, series_title: 'Chew' }),
        missingIssue({ issue_id: 12, number: '2', series_id: 100, series_title: 'Saga' }),
        missingIssue({ issue_id: 22, number: '6', series_id: 200, series_title: 'Chew' }),
        missingIssue({ issue_id: 13, number: '3', series_id: 100, series_title: 'Saga' })
      ])
    );

    // Two groups total — Saga (3 rows) and Chew (2 rows).
    expect(screen.getAllByRole('heading', { level: 2 })).toHaveLength(2);
    expect(screen.getByText(/3 missing/)).toBeInTheDocument();
    expect(screen.getByText(/2 missing/)).toBeInTheDocument();
    // Every issue is rendered exactly once.
    expect(screen.getByText('#1')).toBeInTheDocument();
    expect(screen.getByText('#2')).toBeInTheDocument();
    expect(screen.getByText('#3')).toBeInTheDocument();
    expect(screen.getByText('#5')).toBeInTheDocument();
    expect(screen.getByText('#6')).toBeInTheDocument();
  });
});
