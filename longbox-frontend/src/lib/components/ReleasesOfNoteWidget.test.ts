import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { addCalendarVolumeToPullList, type ReleaseOfNote } from '$lib/api/releases';
import ReleasesOfNoteWidget from './ReleasesOfNoteWidget.svelte';

vi.mock('$lib/api/releases', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/releases')>()),
  addCalendarVolumeToPullList: vi.fn()
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

  it('adds a release to the pull list and drops the row', async () => {
    vi.mocked(addCalendarVolumeToPullList).mockResolvedValue({ series_id: 5 });
    render(ReleasesOfNoteWidget, {
      props: { rows: [note({ cv_volume_id: 100, volume_name: 'Saga' })] }
    });

    await fireEvent.click(screen.getByRole('button', { name: 'Add to pull list' }));

    await waitFor(() => expect(addCalendarVolumeToPullList).toHaveBeenCalledWith(100));
    await waitFor(() => expect(screen.queryByText('Saga')).not.toBeInTheDocument());
    // Last row gone — the widget self-hides.
    expect(screen.queryByText('Releases of note')).not.toBeInTheDocument();
  });
});
