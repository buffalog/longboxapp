// Component test for the release calendar page. The route +page.svelte
// is an ordinary component taking a `data` prop, so it renders in
// isolation with @testing-library/svelte.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  bulkAddCalendarVolumesToPullList,
  getReleaseCalendar,
  type CalendarRow
} from '$lib/api/releases';
import CalendarPage from './+page.svelte';

vi.mock('$lib/api/releases', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/releases')>()),
  getReleaseCalendar: vi.fn(),
  bulkAddCalendarVolumesToPullList: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

function calRow(over: Partial<CalendarRow> = {}): CalendarRow {
  return {
    cv_issue_id: 1,
    issue_number: '1',
    store_date: '2026-05-14',
    cv_volume_id: 100,
    volume_name: 'Saga',
    cover_url: null,
    site_detail_url: 'https://cv/4000-1/',
    series_id: null,
    on_pull_list: false,
    publisher: null,
    ...over
  };
}

function pageData(rows: CalendarRow[]) {
  // `libraryRoot` is merged into the page's `data` prop by the layout
  // load (see src/routes/+layout.ts). Mirroring it here matches the
  // convention used in other route tests (e.g. pull-list/page.test.ts).
  return {
    props: { data: { libraryRoot: null, from: '2026-05-13', to: '2026-05-19', rows } }
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('release calendar page', () => {
  it('renders the calendar rows', () => {
    render(
      CalendarPage,
      pageData([
        calRow({ cv_issue_id: 1, volume_name: 'Saga' }),
        calRow({ cv_issue_id: 2, volume_name: 'Chew', cv_volume_id: 200 })
      ])
    );
    expect(screen.getByText('Saga')).toBeInTheDocument();
    expect(screen.getByText('Chew')).toBeInTheDocument();
  });

  it('shows an empty state when the range has no releases', () => {
    render(CalendarPage, pageData([]));
    expect(screen.getByText('No releases in this range')).toBeInTheDocument();
  });

  it('disables the bulk-add button when no volumes are selected', () => {
    render(
      CalendarPage,
      pageData([calRow({ cv_volume_id: 100, volume_name: 'Saga', on_pull_list: false })])
    );

    expect(screen.getByRole('button', { name: 'Add selected to pull list' })).toBeDisabled();
  });

  it('bulk-adds selected volumes and flips their rows', async () => {
    vi.mocked(bulkAddCalendarVolumesToPullList).mockResolvedValue({
      results: [{ cv_volume_id: 100, status: 'added', series_id: 7 }]
    });
    render(
      CalendarPage,
      pageData([calRow({ cv_volume_id: 100, volume_name: 'Saga', on_pull_list: false })])
    );

    await fireEvent.click(
      screen.getByRole('checkbox', { name: 'Select all addable volumes' })
    );
    await fireEvent.click(
      screen.getByRole('button', { name: 'Add 1 selected to pull list' })
    );

    await waitFor(() =>
      expect(bulkAddCalendarVolumesToPullList).toHaveBeenCalledWith([100])
    );
    await waitFor(() =>
      expect(screen.getByText('On pull list', { selector: 'span' })).toBeInTheDocument()
    );
  });

  it('filters to on-pull-list rows', async () => {
    render(
      CalendarPage,
      pageData([
        calRow({ cv_issue_id: 1, volume_name: 'Subscribed', cv_volume_id: 100, on_pull_list: true }),
        calRow({
          cv_issue_id: 2,
          volume_name: 'Unsubscribed',
          cv_volume_id: 200,
          on_pull_list: false
        })
      ])
    );

    await fireEvent.click(screen.getByRole('tab', { name: 'On pull list' }));

    expect(screen.getByText('Subscribed')).toBeInTheDocument();
    expect(screen.queryByText('Unsubscribed')).not.toBeInTheDocument();
  });

  it('refreshes from ComicVine when Refresh CV is clicked', async () => {
    vi.mocked(getReleaseCalendar).mockResolvedValue([
      calRow({ cv_issue_id: 99, volume_name: 'Fresh Pull' })
    ]);
    render(CalendarPage, pageData([calRow({ cv_issue_id: 1, volume_name: 'Stale' })]));

    await fireEvent.click(screen.getByRole('button', { name: 'Refresh CV' }));

    await waitFor(() =>
      expect(getReleaseCalendar).toHaveBeenCalledWith('2026-05-13', '2026-05-19', true)
    );
    await waitFor(() => expect(screen.getByText('Fresh Pull')).toBeInTheDocument());
  });

  // 6c.5 Item E: publisher grouping.

  it('groups rows by publisher under their own headers', () => {
    render(
      CalendarPage,
      pageData([
        calRow({
          cv_issue_id: 1,
          cv_volume_id: 100,
          volume_name: 'Saga',
          publisher: 'Image Comics'
        }),
        calRow({
          cv_issue_id: 2,
          cv_volume_id: 200,
          volume_name: 'Batman',
          publisher: 'DC Comics'
        }),
        calRow({
          cv_issue_id: 3,
          cv_volume_id: 300,
          volume_name: 'Invincible',
          publisher: 'Image Comics'
        })
      ])
    );
    // Two publisher group headings render, both as <h2>.
    expect(screen.getByRole('heading', { level: 2, name: 'DC Comics' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 2, name: 'Image Comics' })).toBeInTheDocument();
    // All three rows are present somewhere on the page.
    expect(screen.getByText('Saga')).toBeInTheDocument();
    expect(screen.getByText('Batman')).toBeInTheDocument();
    expect(screen.getByText('Invincible')).toBeInTheDocument();
  });

  it('falls back to "Unknown Publisher" for rows whose publisher is null', () => {
    render(
      CalendarPage,
      pageData([
        calRow({ cv_issue_id: 1, cv_volume_id: 100, volume_name: 'Saga', publisher: null }),
        calRow({
          cv_issue_id: 2,
          cv_volume_id: 200,
          volume_name: 'Batman',
          publisher: 'DC Comics'
        })
      ])
    );
    expect(
      screen.getByRole('heading', { level: 2, name: 'Unknown Publisher' })
    ).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 2, name: 'DC Comics' })).toBeInTheDocument();
  });

  it('orders publisher groups alphabetically', () => {
    render(
      CalendarPage,
      pageData([
        calRow({ cv_issue_id: 1, cv_volume_id: 100, volume_name: 'Saga', publisher: 'Image' }),
        calRow({ cv_issue_id: 2, cv_volume_id: 200, volume_name: 'Batman', publisher: 'DC' }),
        calRow({ cv_issue_id: 3, cv_volume_id: 300, volume_name: 'Hellboy', publisher: 'Dark Horse' })
      ])
    );
    const headings = screen
      .getAllByRole('heading', { level: 2 })
      .map((h) => h.textContent?.trim());
    // localeCompare is case-insensitive: "Dark Horse" sorts before
    // "DC" because 'a' < 'c'. That's the right semantic — alphabetical
    // order should ignore case — so the expected order encodes it.
    expect(headings).toEqual(['Dark Horse', 'DC', 'Image']);
  });
});
