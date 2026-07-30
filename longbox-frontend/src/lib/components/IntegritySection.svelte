<script lang="ts">
  // One collapsible finding section. Matches the Tidy sections' shell
  // (rounded border, white, p-4, icon + title + count) so Library Integrity
  // reads as the same app rather than a second visual language.
  import { ChevronDown, ChevronRight } from 'lucide-svelte';
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    count: number;
    /** Shown next to the count. Use for "none found" vs "not yet analyzed" —
     * a bare 0 cannot tell a reader which one it is. */
    note?: string;
    /** Renders the count in a warning tone. */
    warn?: boolean;
    open?: boolean;
    children: Snippet;
  }

  let { title, count, note, warn = false, open = $bindable(false), children }: Props = $props();
</script>

<section class="rounded-lg border border-slate-200 bg-white">
  <button
    type="button"
    class="flex w-full items-center gap-2 p-4 text-left"
    onclick={() => (open = !open)}
    aria-expanded={open}
  >
    {#if open}
      <ChevronDown class="size-4 shrink-0 text-slate-400" aria-hidden="true" />
    {:else}
      <ChevronRight class="size-4 shrink-0 text-slate-400" aria-hidden="true" />
    {/if}
    <h2 class="text-base font-semibold">{title}</h2>
    <span
      class="rounded px-1.5 py-0.5 text-sm font-medium {warn && count > 0
        ? 'bg-amber-50 text-amber-800'
        : 'bg-slate-100 text-slate-600'}"
    >
      {count}
    </span>
    {#if note}
      <span class="text-sm font-normal text-slate-500">{note}</span>
    {/if}
  </button>

  {#if open}
    <div class="border-t border-slate-100 p-4 pt-3">
      {@render children()}
    </div>
  {/if}
</section>
