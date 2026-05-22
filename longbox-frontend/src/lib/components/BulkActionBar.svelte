<script lang="ts">
  // The select-all + "N selected" + action-button bar shared by every
  // bulk-action surface. Pairs with createSelection. The consumer keeps
  // its own per-row checkboxes — table vs list row markup differs — and
  // supplies the surface-specific action button(s) via the `action`
  // snippet.
  import type { Snippet } from 'svelte';

  interface Props {
    /** Number of selected rows — the "N selected" readout. */
    count: number;
    /** Every shown row is selected — drives the header checkbox. */
    allSelected: boolean;
    /** Some but not all shown rows are selected — indeterminate state. */
    someSelected: boolean;
    /** Toggle select-all over the currently-shown rows. */
    onToggleAll: () => void;
    /** Accessible label for the select-all checkbox. */
    selectAllLabel: string;
    /** The surface-specific action button(s) — typically disabled at count 0. */
    action: Snippet;
  }

  let { count, allSelected, someSelected, onToggleAll, selectAllLabel, action }: Props =
    $props();
</script>

<div
  class="mb-2 flex items-center justify-between rounded-md border border-slate-200 bg-slate-50 px-3 py-2"
>
  <label class="flex items-center gap-2 text-sm text-slate-600">
    <input
      type="checkbox"
      class="rounded border-slate-300"
      checked={allSelected}
      indeterminate={someSelected && !allSelected}
      onchange={onToggleAll}
      aria-label={selectAllLabel}
    />
    {count} selected
  </label>
  {@render action()}
</div>
