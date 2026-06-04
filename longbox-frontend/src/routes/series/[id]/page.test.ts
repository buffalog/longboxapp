// Component test for the series detail page. Locks the cv_id-aware
// rendering: a CV-linked series shows Refresh + the "Hit Refresh"
// hint; a shallow series (cv_id null) hides Refresh and shows the
// scan-attaches-files hint instead. Plugs the test-coverage gap that
// let the shallow-Refresh-400 bug ship in the first place — F4 from
// the hot-fix kickoff (the page had no tests, so the conditional
// branch was never exercised either way).
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { SeriesDetail } from '$lib/types';
import type { PullEntry } from '$lib/api/pull';
import { deleteSeries } from '$lib/api/series';
import Page from './+page.svelte';

vi.mock('$lib/api/series', () => ({
  refreshSeries: vi.fn(),
  deleteSeries: vi.fn()
}));

vi.mock('$app/navigation', () => ({
  goto: vi.fn(),
  invalidateAll: vi.fn()
}));

function seriesDetail(over: Partial<SeriesDetail> = {}): SeriesDetail {
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
    issues: [],
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
    expect(
      screen.getByText(/Hit Refresh to fetch from ComicVine/)
    ).toBeInTheDocument();
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
    render(Page, pageData(seriesDetail({ title: 'Yearless', start_year: null })));

    await fireEvent.click(screen.getByRole('button', { name: /Delete series/ }));
    // Bare title, no `(YYYY)` suffix.
    expect(screen.getByText('Yearless/')).toBeInTheDocument();
    expect(screen.queryByText(/\(\d{4}\)\//)).not.toBeInTheDocument();
  });

  it('hides Refresh and shows the shallow empty-issues hint for a cv_id-NULL series', () => {
    // A.9 shallow-series UX hot-fix: the Refresh button calls a CV
    // endpoint that 400s when cv_id is NULL, and the "Hit Refresh"
    // hint actively directs the user at the broken affordance.
    // Shallow series get neither.
    render(Page, pageData(seriesDetail({ cv_id: null, publisher: null })));

    expect(screen.queryByRole('button', { name: /Refresh/ })).not.toBeInTheDocument();
    expect(
      screen.queryByText(/Hit Refresh to fetch from ComicVine/)
    ).not.toBeInTheDocument();
    expect(
      screen.getByText(/will appear here as the next scan parses and attaches them/)
    ).toBeInTheDocument();
  });
});
