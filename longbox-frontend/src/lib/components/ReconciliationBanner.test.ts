import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ReconciliationBanner from './ReconciliationBanner.svelte';

const KEY = 'longbox.reconcileBannerDismissed';

// jsdom under Vitest doesn't supply a full `Storage`, so stub a clean
// in-memory one per test — both the component and the assertions read
// through it.
function makeStorage() {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
    key: (i: number) => [...m.keys()][i] ?? null
  };
}

beforeEach(() => {
  vi.stubGlobal('localStorage', makeStorage());
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('ReconciliationBanner', () => {
  it('renders nothing when both counts are zero', () => {
    render(ReconciliationBanner, { props: { transitionCount: 0, untrackedCount: 0 } });
    expect(screen.queryByRole('link', { name: 'Review' })).not.toBeInTheDocument();
  });

  it('renders both sentences with plural wording', () => {
    render(ReconciliationBanner, { props: { transitionCount: 5, untrackedCount: 3 } });
    expect(
      screen.getByText('5 series lost their files. 3 untracked folders detected.')
    ).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Review' })).toHaveAttribute(
      'href',
      '/library/tidy'
    );
  });

  it('uses singular wording for a count of one', () => {
    render(ReconciliationBanner, { props: { transitionCount: 1, untrackedCount: 1 } });
    expect(
      screen.getByText('1 series lost its files. 1 untracked folder detected.')
    ).toBeInTheDocument();
  });

  it('omits the zero count when only one kind is present', () => {
    render(ReconciliationBanner, { props: { transitionCount: 0, untrackedCount: 4 } });
    expect(screen.getByText('4 untracked folders detected.')).toBeInTheDocument();
  });

  it('dismiss hides the banner and stores the count signature', async () => {
    render(ReconciliationBanner, { props: { transitionCount: 5, untrackedCount: 3 } });
    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByRole('link', { name: 'Review' })).not.toBeInTheDocument();
    expect(localStorage.getItem(KEY)).toBe('5:3');
  });

  it('stays dismissed while the counts match the stored signature', () => {
    localStorage.setItem(KEY, '5:3');
    render(ReconciliationBanner, { props: { transitionCount: 5, untrackedCount: 3 } });
    expect(screen.queryByRole('link', { name: 'Review' })).not.toBeInTheDocument();
  });

  it('reappears when a count changes from the dismissed signature', () => {
    localStorage.setItem(KEY, '5:3');
    render(ReconciliationBanner, { props: { transitionCount: 7, untrackedCount: 3 } });
    expect(screen.getByRole('link', { name: 'Review' })).toBeInTheDocument();
  });
});
