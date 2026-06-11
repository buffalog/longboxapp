// Component test for SeriesCard. Focused on the badge split between
// genuinely-missing and solicited (pre-release) issues — owning every
// issue that has shipped reads as complete even when future-dated
// issues are already in the catalog.
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
    ...over
  };
}

function badge(container: HTMLElement): HTMLElement {
  const el = container.querySelector('span.rounded-full');
  if (!el) throw new Error('badge not found');
  return el as HTMLElement;
}

describe('SeriesCard badge', () => {
  it('reads complete (green) when owned >= available even with solicited issues', () => {
    // The reproduction: Absolute Martian Manhunter — 10 owned, 1 solicited.
    // Pre-fix: orange 10/11. Post-fix: green 10/10 · +1.
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          title: 'Absolute Martian Manhunter',
          total_count: 11,
          owned_count: 10,
          missing_count: 1,
          solicited_count: 1
        })
      }
    });
    const b = badge(container);
    expect(b.className).toContain('status-owned');
    expect(b.textContent).toMatch(/10\/10/);
    expect(b.textContent).toMatch(/\+1/);
  });

  it('reads incomplete (orange) when a shipped issue is genuinely missing', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          total_count: 10,
          owned_count: 8,
          missing_count: 2,
          solicited_count: 0
        })
      }
    });
    const b = badge(container);
    expect(b.className).toContain('status-needs_review');
    expect(b.textContent).toMatch(/8\/10/);
    expect(b.textContent).not.toMatch(/\+/);
  });

  it('reads unmatched (red) when nothing is owned', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          total_count: 5,
          owned_count: 0,
          missing_count: 5,
          solicited_count: 0
        })
      }
    });
    expect(badge(container).className).toContain('status-unmatched');
  });

  it('reads neutral (slate) when no issues exist at all', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({ total_count: 0, owned_count: 0 })
      }
    });
    expect(badge(container).className).toContain('bg-slate-100');
  });

  it('omits the +N suffix when no solicited issues exist', () => {
    const { container } = render(SeriesCard, {
      props: {
        series: series({
          total_count: 3,
          owned_count: 3,
          missing_count: 0,
          solicited_count: 0
        })
      }
    });
    const b = badge(container);
    expect(b.textContent).toMatch(/3\/3/);
    expect(b.textContent).not.toMatch(/\+/);
  });
});
