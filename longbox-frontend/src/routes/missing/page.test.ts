// Regression tests for the /missing page's series grouping. The
// original consecutive-merge loop crashed Svelte's keyed each block
// with each_key_duplicate when the API response interleaved rows from
// two distinct series ids that shared a title (e.g. multiple "The
// Department of Truth" volumes). The Map-based grouper handles any
// sort order.
import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MissingIssue, MissingResponse } from '$lib/api/missing';
import MissingPage from './+page.svelte';

// goto is called only when a sort/filter control changes; not exercised
// here but the import is reachable, so stub it for safety.
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

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
