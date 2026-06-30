// Component test for SeriesCard. Covers the issue-status taxonomy the badge
// renders: Available (owned), Missing (released, not owned → red), Solicited
// (future → blue +N), and the purple "complete collection" celebration
// (finished run, fully owned, nothing upcoming).
import { render } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import type { SeriesWithCounts } from '$lib/types';
import SeriesCard from './SeriesCard.svelte';

function series(over: Partial<SeriesWithCounts> = {}): SeriesWithCounts {
  return {
    id: 1,
    cv_id: null,
    metron_id: null,
    title: 'Test Series',
    sort_title: 'test series',
    start_year: 2026,
    publisher: 'Test',
    description: null,
    cover_url: null,
    created_at: '2026-01-01T00:00:00',
    updated_at: '2026-01-01T00:00:00',
    total_count: 0,
    owned_count: 0,
    needs_review_count: 0,
    ignored_count: 0,
    unmatched_count: 0,
    missing_count: 0,
    solicited_count: 0,
    finished: false,
    ...over
  };
}

const fraction = (c: HTMLElement) => c.querySelector('[data-testid="badge-fraction"]');
const solicited = (c: HTMLElement) => c.querySelector('[data-testid="badge-solicited"]');
const empty = (c: HTMLElement) => c.querySelector('[data-testid="badge-empty"]');

describe('SeriesCard badge', () => {
  it('suppresses the X/Y fraction and shows only +N (blue) when no released issues exist', () => {
    // The bug: Absolute Catwoman — 0 released, 1 solicited. Pre-fix: red
    // "0/0 · +1". Post-fix: no fraction, just a blue "+1".
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          title: 'Absolute Catwoman',
          total_count: 1,
          owned_count: 0,
          missing_count: 1,
          solicited_count: 1
        })
      }
    });
    expect(fraction(container)).toBeNull();
    const s = solicited(container)!;
    expect(s).not.toBeNull();
    expect(s.textContent).toMatch(/\+1/);
    expect(s.className).toContain('status-solicited');
  });

  it('shows X/Y in red when a released issue is genuinely missing, +N in blue', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          total_count: 11,
          owned_count: 8,
          missing_count: 3,
          solicited_count: 1
        })
      }
    });
    const f = fraction(container)!;
    expect(f.textContent).toMatch(/8\/10/);
    expect(f.className).toContain('status-unmatched');
    expect(solicited(container)!.className).toContain('status-solicited');
  });

  it('shows X/Y in red when nothing is owned but issues have shipped', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({ total_count: 5, owned_count: 0, missing_count: 5, solicited_count: 0 })
      }
    });
    const f = fraction(container)!;
    expect(f.textContent).toMatch(/0\/5/);
    expect(f.className).toContain('status-unmatched');
  });

  it('shows X/Y in neutral (not green, not purple) when all shipped issues are owned but the run is unconfirmed', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          total_count: 11,
          owned_count: 10,
          missing_count: 1,
          solicited_count: 1
        })
      }
    });
    const f = fraction(container)!;
    expect(f.textContent).toMatch(/10\/10/);
    expect(f.className).toContain('text-slate-600');
    expect(f.className).not.toContain('status-finished');
    expect(solicited(container)!.textContent).toMatch(/\+1/);
  });

  it('shows X/Y in PURPLE when the series is finished and fully owned with nothing upcoming', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          total_count: 12,
          owned_count: 12,
          missing_count: 0,
          solicited_count: 0,
          finished: true
        })
      }
    });
    const f = fraction(container)!;
    expect(f.textContent).toMatch(/12\/12/);
    expect(f.className).toContain('status-finished');
    expect(solicited(container)).toBeNull();
  });

  it('stays red (not purple) for a finished series that is missing issues', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          total_count: 12,
          owned_count: 10,
          missing_count: 2,
          solicited_count: 0,
          finished: true
        })
      }
    });
    const f = fraction(container)!;
    expect(f.textContent).toMatch(/10\/12/);
    expect(f.className).toContain('status-unmatched');
  });

  it('stays neutral (not purple) when fully owned but finished is unknown', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({ total_count: 5, owned_count: 5, solicited_count: 0 })
      }
    });
    const f = fraction(container)!;
    expect(f.textContent).toMatch(/5\/5/);
    expect(f.className).not.toContain('status-finished');
    expect(f.className).toContain('text-slate-600');
  });

  it('shows a neutral 0/0 placeholder when the series has no issues at all', () => {
    const { container } = render(SeriesCard, {
      props: { series: series({ total_count: 0, owned_count: 0 }) }
    });
    const e = empty(container)!;
    expect(e).not.toBeNull();
    expect(e.textContent).toMatch(/0\/0/);
    expect(fraction(container)).toBeNull();
    expect(solicited(container)).toBeNull();
  });
});
