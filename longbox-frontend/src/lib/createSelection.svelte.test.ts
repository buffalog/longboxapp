import { describe, expect, it } from 'vitest';
import { createSelection } from './createSelection.svelte';

describe('createSelection', () => {
  it('starts empty', () => {
    const sel = createSelection<number>();
    expect(sel.size).toBe(0);
    expect(sel.has(1)).toBe(false);
  });

  it('toggles an id on and off', () => {
    const sel = createSelection<number>();
    sel.toggle(7);
    expect(sel.has(7)).toBe(true);
    expect(sel.size).toBe(1);
    sel.toggle(7);
    expect(sel.has(7)).toBe(false);
    expect(sel.size).toBe(0);
  });

  it('toggleAll selects every id when some are unselected', () => {
    const sel = createSelection<number>();
    sel.toggle(1);
    sel.toggleAll([1, 2, 3]);
    expect(sel.size).toBe(3);
    expect([1, 2, 3].every((id) => sel.has(id))).toBe(true);
  });

  it('toggleAll clears when every id is already selected', () => {
    const sel = createSelection<number>();
    sel.toggleAll([1, 2, 3]);
    sel.toggleAll([1, 2, 3]);
    expect(sel.size).toBe(0);
  });

  it('allSelected / someSelected reflect the shown rows', () => {
    const sel = createSelection<number>();
    expect(sel.allSelected([])).toBe(false); // empty list is never "all"
    expect(sel.someSelected([1, 2])).toBe(false);
    sel.toggle(1);
    expect(sel.someSelected([1, 2])).toBe(true);
    expect(sel.allSelected([1, 2])).toBe(false);
    sel.toggle(2);
    expect(sel.allSelected([1, 2])).toBe(true);
  });

  it('discard removes an id only when it is selected', () => {
    const sel = createSelection<number>();
    sel.toggle(1);
    sel.toggle(2);
    sel.discard(1);
    expect(sel.has(1)).toBe(false);
    expect(sel.has(2)).toBe(true);
    expect(sel.size).toBe(1);
    // Discarding an id that isn't selected is a no-op.
    sel.discard(99);
    expect(sel.size).toBe(1);
  });

  it('clear drops all selections', () => {
    const sel = createSelection<string>();
    sel.toggleAll(['a', 'b']);
    sel.clear();
    expect(sel.size).toBe(0);
  });
});
