import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import type { PullThisWeek } from '$lib/api/releases';
import ThisWeeksPullsWidget from './ThisWeeksPullsWidget.svelte';

function pull(over: Partial<PullThisWeek> = {}): PullThisWeek {
  return {
    cv_issue_id: 1,
    issue_number: '60',
    store_date: '2026-05-14',
    cv_volume_id: 100,
    volume_name: 'Saga',
    cover_url: null,
    site_detail_url: 'https://cv/4000-1/',
    ...over
  };
}

describe('ThisWeeksPullsWidget', () => {
  it('renders the issues shipping this week', () => {
    render(ThisWeeksPullsWidget, {
      props: {
        rows: [
          pull({ cv_issue_id: 1, volume_name: 'Saga' }),
          pull({ cv_issue_id: 2, volume_name: 'Chew' })
        ]
      }
    });
    expect(screen.getByText("This week's pulls")).toBeInTheDocument();
    expect(screen.getByText('Saga')).toBeInTheDocument();
    expect(screen.getByText('Chew')).toBeInTheDocument();
  });

  it('renders nothing when no issues ship this week', () => {
    render(ThisWeeksPullsWidget, { props: { rows: [] } });
    expect(screen.queryByText("This week's pulls")).not.toBeInTheDocument();
  });
});
