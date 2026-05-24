// Component test for the series detail page. Locks the cv_id-aware
// rendering: a CV-linked series shows Refresh + the "Hit Refresh"
// hint; a shallow series (cv_id null) hides Refresh and shows the
// scan-attaches-files hint instead. Plugs the test-coverage gap that
// let the shallow-Refresh-400 bug ship in the first place — F4 from
// the hot-fix kickoff (the page had no tests, so the conditional
// branch was never exercised either way).
import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { SeriesDetail } from '$lib/types';
import type { PullEntry } from '$lib/api/pull';
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
