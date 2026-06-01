import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  addCalendarVolumeToPullList,
  bulkAddCalendarVolumesToPullList,
  type ReleaseOfNote
} from '$lib/api/releases';
import ReleasesOfNoteWidget from './ReleasesOfNoteWidget.svelte';

vi.mock('$lib/api/releases', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/releases')>()),
  addCalendarVolumeToPullList: vi.fn(),
  bulkAddCalendarVolumesToPullList: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

function note(over: Partial<ReleaseOfNote> = {}): ReleaseOfNote {
  return {
    cv_volume_id: 100,
    volume_name: 'Saga',
    cover_url: null,
    site_detail_url: 'https://cv/4050-100/',
    issue_count: 1,
    ...over
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('ReleasesOfNoteWidget', () => {
  it('renders the releases', () => {
    render(ReleasesOfNoteWidget, {
      props: {
        rows: [note({ volume_name: 'Saga' }), note({ cv_volume_id: 200, volume_name: 'Chew' })]
      }
    });
    expect(screen.getByText('Releases of note')).toBeInTheDocument();
    expect(screen.getByText('Saga')).toBeInTheDocument();
    expect(screen.getByText('Chew')).toBeInTheDocument();
  });

  it('renders nothing when there are no releases', () => {
    render(ReleasesOfNoteWidget, { props: { rows: [] } });
    expect(screen.queryByText('Releases of note')).not.toBeInTheDocument();
  });

  it('adds a release and badges it in place — the row is not removed', async () => {
    vi.mocked(addCalendarVolumeToPullList).mockResolvedValue({ series_id: 5 });
    render(ReleasesOfNoteWidget, {
      props: { rows: [note({ cv_volume_id: 100, volume_name: 'Saga' })] }
    });

    await fireEvent.click(screen.getByRole('button', { name: 'Add to pull list' }));

    await waitFor(() =>
      expect(addCalendarVolumeToPullList).toHaveBeenCalledWith({ cv_volume_id: 100 })
    );
    // The row stays — with the "On pull list" badge in place of the button.
    expect(screen.getByText('Saga')).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByText('On pull list', { selector: 'span' })).toBeInTheDocument()
    );
    expect(
      screen.queryByRole('button', { name: 'Add to pull list' })
    ).not.toBeInTheDocument();
    // Widget stays visible so the user sees their action land.
    expect(screen.getByText('Releases of note')).toBeInTheDocument();
  });

  it('bulk-adds selected releases and badges them in place', async () => {
    vi.mocked(bulkAddCalendarVolumesToPullList).mockResolvedValue({
      results: [
        { cv_volume_id: 100, metron_series_id: null, status: 'added', series_id: 5 },
        { cv_volume_id: 200, metron_series_id: null, status: 'added', series_id: 6 }
      ]
    });
    render(ReleasesOfNoteWidget, {
      props: {
        rows: [
          note({ cv_volume_id: 100, volume_name: 'Saga' }),
          note({ cv_volume_id: 200, volume_name: 'Chew' })
        ]
      }
    });

    await fireEvent.click(
      screen.getByRole('checkbox', { name: 'Select all releases of note' })
    );
    await fireEvent.click(
      screen.getByRole('button', { name: 'Add 2 selected to pull list' })
    );

    await waitFor(() =>
      expect(bulkAddCalendarVolumesToPullList).toHaveBeenCalledWith([
        { cv_volume_id: 100 },
        { cv_volume_id: 200 }
      ])
    );
    await waitFor(() =>
      expect(screen.getAllByText('On pull list', { selector: 'span' })).toHaveLength(2)
    );
  });
});
