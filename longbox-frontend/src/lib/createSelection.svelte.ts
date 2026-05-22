// Reactive multi-select state for bulk-action surfaces — the release
// calendar, the of-note widget, and (from A.9 Step 6) Library Tidy.
// Pairs with the BulkActionBar component.
//
// Svelte 5's `$state` does not deeply track Set mutation, so every
// mutation reassigns the whole binding — the same idiom the pre-helper
// /library/tidy code used inline.

/** Reactive selection over a set of ids of type `T` (a row key). */
export function createSelection<T>() {
  let ids = $state<Set<T>>(new Set());

  return {
    /** The currently-selected ids. */
    get selected(): Set<T> {
      return ids;
    },
    /** How many ids are selected. */
    get size(): number {
      return ids.size;
    },
    /** Is `id` selected? */
    has(id: T): boolean {
      return ids.has(id);
    },
    /** Flip one id's selected state. */
    toggle(id: T): void {
      const next = new Set(ids);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      ids = next;
    },
    /** Select every id in `all` if any is currently unselected;
     *  otherwise clear. Mirrors a select-all checkbox against the
     *  currently-shown rows. */
    toggleAll(all: T[]): void {
      ids = all.length > 0 && all.every((id) => ids.has(id)) ? new Set() : new Set(all);
    },
    /** Drop every selection. */
    clear(): void {
      ids = new Set();
    },
    /** True when every id in `all` is selected (and `all` is non-empty). */
    allSelected(all: T[]): boolean {
      return all.length > 0 && all.every((id) => ids.has(id));
    },
    /** True when at least one id in `all` is selected. */
    someSelected(all: T[]): boolean {
      return all.some((id) => ids.has(id));
    }
  };
}
