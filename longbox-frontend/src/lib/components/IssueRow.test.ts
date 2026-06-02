// Component test for IssueRow. Focused on the per-issue Search button
// surface added for the missing-issue search workflow — visibility
// rules per status, click behavior, debounce, error handling.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api/client';
import { searchIssueNow } from '$lib/api/pull';
import type { IssueWithFile } from '$lib/types';
import IssueRow from './IssueRow.svelte';

vi.mock('$lib/api/pull', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/pull')>()),
  searchIssueNow: vi.fn()
}));

vi.mock('$lib/stores/toast.svelte', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() }
}));

function issue(over: Partial<IssueWithFile> = {}): IssueWithFile {
  return {
    id: 7,
    series_id: 42,
    cv_issue_id: null,
    metron_issue_id: null,
    number: '4',
    title: 'The Fourth Issue',
    cover_date: '2024-01-15',
    summary: null,
    cover_url: null,
    created_at: '2026-01-01T00:00:00',
    updated_at: '2026-01-01T00:00:00',
    file: null,
    ...over
  } as IssueWithFile;
}

function renderRow(props: { issue: IssueWithFile; seriesId?: number }) {
  return render(IssueRow, { props });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('IssueRow Search button', () => {
  it('renders for a Missing issue when seriesId is provided', () => {
    // file === null + past cover date → 'missing' on the row's
    // derived status. seriesId present → button surfaces.
    renderRow({ issue: issue({ file: null }), seriesId: 42 });
    expect(
      screen.getByRole('button', { name: /Search for issue 4/i })
    ).toBeInTheDocument();
  });

  it('hides for an Owned issue', () => {
    renderRow({
      issue: issue({
        file: {
          id: 1,
          path_relative: 'Saga (2012)/Saga 004.cbz',
          status: 'owned',
          is_present: true
        }
      }),
      seriesId: 42
    });
    expect(
      screen.queryByRole('button', { name: /Search for issue 4/i })
    ).not.toBeInTheDocument();
  });

  it('hides for a Solicited issue (not shipped yet)', () => {
    // file === null + future cover date → 'solicited'. The user
    // can't search for an issue that hasn't been published.
    renderRow({
      issue: issue({ file: null, cover_date: '2099-01-01' }),
      seriesId: 42
    });
    expect(
      screen.queryByRole('button', { name: /Search for issue 4/i })
    ).not.toBeInTheDocument();
  });

  it('hides when seriesId is omitted (context lacks a parent series)', () => {
    // Defensive: IssueRow rendered without a parent series (e.g.,
    // future needs-attention listing) can't construct the endpoint
    // URL, so the button is hidden rather than rendered broken.
    renderRow({ issue: issue({ file: null }) });
    expect(
      screen.queryByRole('button', { name: /Search for issue/i })
    ).not.toBeInTheDocument();
  });

  it('clicking calls searchIssueNow with the parent seriesId and issue.id', async () => {
    vi.mocked(searchIssueNow).mockResolvedValue(undefined);
    renderRow({ issue: issue({ id: 7, number: '4' }), seriesId: 42 });

    await fireEvent.click(
      screen.getByRole('button', { name: /Search for issue 4/i })
    );

    await waitFor(() => expect(searchIssueNow).toHaveBeenCalledTimes(1));
    expect(searchIssueNow).toHaveBeenCalledWith(42, 7);
  });

  it('disables the button after a click (debounce)', async () => {
    vi.mocked(searchIssueNow).mockResolvedValue(undefined);
    renderRow({ issue: issue(), seriesId: 42 });

    const btn = screen.getByRole('button', { name: /Search for issue 4/i });
    await fireEvent.click(btn);

    // The 15s timer is still running well after the API resolves —
    // the button stays disabled to absorb impatient re-clicks.
    await waitFor(() => expect(btn).toBeDisabled());
  });

  it('shows a warning toast on a 404 error', async () => {
    const { toast } = await import('$lib/stores/toast.svelte');
    vi.mocked(searchIssueNow).mockRejectedValue(
      new ApiError(404, 'not_found.issue', 'issue 7 not found')
    );
    renderRow({ issue: issue(), seriesId: 42 });

    await fireEvent.click(
      screen.getByRole('button', { name: /Search for issue 4/i })
    );
    await waitFor(() =>
      expect(toast.warning).toHaveBeenCalledWith(
        expect.stringContaining('issue 7 not found')
      )
    );
  });
});
