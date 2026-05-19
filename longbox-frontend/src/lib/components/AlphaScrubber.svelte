<script lang="ts" generics="T">
  import { onMount, untrack } from 'svelte';
  import { ALPHA_BUCKETS, bucketLetter } from '$lib/scrubber';

  interface Props<T> {
    /** Items currently rendered in the parent list (post-filter). The
     *  scrubber buckets these, dims letters with no entries, and jumps
     *  to the first entry per bucket on click. */
    items: T[];
    /** Extract the sort-key from an item (e.g., `s => s.sort_title`). */
    getSortKey: (item: T) => string;
    /** Extract the DOM id of the item's row element (must match the
     *  `id={...}` the parent puts on each rendered row). */
    getElementId: (item: T) => string;
    /** Bind this to the list's container element so the scrubber can
     *  measure scrollable height for the contextual auto-hide threshold
     *  (visible when listEl.offsetHeight > 1.5 * window.innerHeight). */
    listEl?: HTMLElement | null;
  }

  let { items, getSortKey, getElementId, listEl = null }: Props<T> = $props();

  // Bucket → first matching element id. Recomputed when items changes.
  // `null` means no entries for this bucket → dim, not clickable.
  const firstIdByBucket = $derived.by(() => {
    const map: Record<string, string | null> = {};
    for (const letter of ALPHA_BUCKETS) map[letter] = null;
    for (const item of items) {
      const bucket = bucketLetter(getSortKey(item));
      if (map[bucket] === null) {
        map[bucket] = getElementId(item);
      }
    }
    return map;
  });

  // Contextual auto-hide: scrubber visible only when the list exceeds
  // ~1.5 viewports of scrollable content. Recomputed on resize + on
  // list-content size changes via ResizeObserver.
  let visible = $state(false);
  let viewportHeight = $state(typeof window !== 'undefined' ? window.innerHeight : 800);

  function recompute(): void {
    if (!listEl) {
      visible = false;
      return;
    }
    visible = listEl.offsetHeight > 1.5 * viewportHeight;
  }

  // Re-measure whenever the bound list element appears / changes, and on
  // every items mutation (filter narrowed/widened the list).
  $effect(() => {
    items; // dep
    listEl; // dep
    untrack(() => recompute());
  });

  onMount(() => {
    const onResize = () => {
      viewportHeight = window.innerHeight;
      recompute();
    };
    window.addEventListener('resize', onResize);

    let observer: ResizeObserver | null = null;
    if (listEl && typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver(() => recompute());
      observer.observe(listEl);
    }

    return () => {
      window.removeEventListener('resize', onResize);
      observer?.disconnect();
    };
  });

  function jumpTo(letter: string): void {
    const id = firstIdByBucket[letter];
    if (!id) return;
    const el = document.getElementById(id);
    if (!el) return;
    el.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }
</script>

{#if visible}
  <!-- Fixed strip on the right edge, vertically centered. z-30 sits
       below the (future) sticky nav (z-50) and well below modals. -->
  <nav
    aria-label="Alphabetical jump"
    class="fixed right-2 top-1/2 z-30 hidden -translate-y-1/2 select-none flex-col items-center gap-0 rounded bg-white/80 px-1 py-1 text-[10px] font-medium leading-none shadow-sm backdrop-blur-sm sm:flex"
  >
    {#each ALPHA_BUCKETS as letter (letter)}
      {@const hasEntries = firstIdByBucket[letter] !== null}
      {#if hasEntries}
        <button
          type="button"
          class="w-4 py-0.5 rounded text-slate-700 hover:bg-blue-50 hover:text-blue-600 focus:outline-none focus:ring-2 focus:ring-blue-500"
          onclick={() => jumpTo(letter)}
        >{letter}</button>
      {:else}
        <span class="w-4 py-0.5 text-slate-300" aria-hidden="true">{letter}</span>
      {/if}
    {/each}
  </nav>
{/if}
