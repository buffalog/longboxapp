// The API sends RFC 3339 ("…Z"); the duration used to append its own "Z"
// and went blank on that shape.
import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import type { ScanRun } from '$lib/types';
import ScanReportCard from './ScanReportCard.svelte';

const report: ScanRun = {
  id: 1,
  library_root_id: 1,
  started_at: '2026-10-28T16:17:13Z',
  finished_at: '2026-10-28T16:18:52Z',
  files_seen: 0,
  files_added: 0,
  files_updated: 0,
  files_matched: 0,
  files_needs_review: 0,
  files_unmatched: 0,
  status: 'completed',
  error_message: null,
  kind: 'full'
};

describe('ScanReportCard', () => {
  it('shows the duration of an RFC 3339 scan run', () => {
    render(ScanReportCard, { props: { report } });
    expect(screen.getByText(/· 1m 39s/)).toBeInTheDocument();
  });
});
