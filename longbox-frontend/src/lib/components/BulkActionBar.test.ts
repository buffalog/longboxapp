import { fireEvent, render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { describe, expect, it, vi } from 'vitest';
import BulkActionBar from './BulkActionBar.svelte';

// A minimal action-button snippet to stand in for the surface-specific
// content the consumer supplies.
const action = createRawSnippet(() => ({
  render: () => '<button type="button">Add selected</button>'
}));

function props(over: Record<string, unknown> = {}) {
  return {
    props: {
      count: 0,
      allSelected: false,
      someSelected: false,
      onToggleAll: vi.fn(),
      selectAllLabel: 'Select all rows',
      action,
      ...over
    }
  };
}

describe('BulkActionBar', () => {
  it('shows the selected count and renders the action snippet', () => {
    render(BulkActionBar, props({ count: 3 }));
    expect(screen.getByText('3 selected')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add selected' })).toBeInTheDocument();
  });

  it('checks the select-all box when every row is selected', () => {
    render(BulkActionBar, props({ allSelected: true }));
    expect(screen.getByRole('checkbox', { name: 'Select all rows' })).toBeChecked();
  });

  it('is indeterminate when only some rows are selected', () => {
    render(BulkActionBar, props({ someSelected: true, allSelected: false }));
    const box = screen.getByRole('checkbox', { name: 'Select all rows' }) as HTMLInputElement;
    expect(box.indeterminate).toBe(true);
  });

  it('fires onToggleAll when the select-all box is clicked', async () => {
    const onToggleAll = vi.fn();
    render(BulkActionBar, props({ onToggleAll }));
    await fireEvent.click(screen.getByRole('checkbox', { name: 'Select all rows' }));
    expect(onToggleAll).toHaveBeenCalledOnce();
  });
});
