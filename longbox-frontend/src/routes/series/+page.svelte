<script lang="ts">
  import { BookOpen } from 'lucide-svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import SeriesCard from '$lib/components/SeriesCard.svelte';
  import AlphaScrubber from '$lib/components/AlphaScrubber.svelte';
  import Button from '$lib/components/Button.svelte';
  import type { SeriesSort } from './+page';
  import type { SeriesWithCounts } from '$lib/types';

  let { data } = $props();

  let filter = $state('');
  // Sort dropdown — Task 5a. "completion" surfaces almost-complete series
  // at the top (natural prioritization signal for what to acquire next).
  // Client-side sort: the typical Phase A library has tens of series;
  // server-side sort is on the future-scaling deferred queue.
  //
  // Initialized from `?sort=` (validated in +page.ts) so a shared link
  // like /series?sort=completion lands with the right view. On change
  // we mirror the new value back into the URL via history.replaceState
  // (not goto) — sort is a pure client-side $derived transform, no need
  // to re-run load().
  let sort = $state<SeriesSort>(data.sort);
  let listEl: HTMLElement | null = $state(null);

  function setSort(next: SeriesSort): void {
    sort = next;
    const url = new URL(window.location.href);
    if (next === 'name') {
      url.searchParams.delete('sort');
    } else {
      url.searchParams.set('sort', next);
    }
    window.history.replaceState(null, '', url.pathname + url.search);
  }

  function completionPct(total: number, owned: number): number {
    if (total === 0) return 0;
    return (owned / total) * 100;
  }

  const visible = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    const list = q ? data.series.filter((s) => s.title.toLowerCase().includes(q)) : data.series;
    const sorted = [...list];
    switch (sort) {
      case 'year':
        sorted.sort((a, b) => (b.start_year ?? 0) - (a.start_year ?? 0));
        break;
      case 'added':
        // Newest first by created_at.
        sorted.sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
        break;
      case 'completion':
        // Descending completion %. Series with no total go to the
        // bottom so the high-signal "missing one or two issues" rows
        // bubble up.
        sorted.sort((a, b) => {
          const ap = completionPct(a.total_count, a.owned_count);
          const bp = completionPct(b.total_count, b.owned_count);
          return bp - ap;
        });
        break;
      case 'name':
      default:
        sorted.sort((a, b) => a.sort_title.localeCompare(b.sort_title));
        break;
    }
    return sorted;
  });
</script>

<header class="mb-4 flex items-center justify-between gap-3">
  <h1 class="text-2xl font-bold">Series</h1>
  <a href="/add">
    <Button>Add series</Button>
  </a>
</header>

{#if data.series.length === 0}
  <EmptyState
    icon={BookOpen}
    title="No series yet"
    message="Add your first series to start tracking what's missing on disk."
  >
    {#snippet cta()}
      <a href="/add"><Button>Add series</Button></a>
    {/snippet}
  </EmptyState>
{:else}
  <div class="mb-4 flex flex-wrap gap-3">
    <input
      type="search"
      class="flex-1 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      placeholder="Filter by title…"
      bind:value={filter}
    />
    <label class="inline-flex items-center gap-2 text-sm">
      <span class="text-slate-600">Sort</span>
      <select
        class="rounded-md border border-slate-300 bg-white px-2 py-1 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        value={sort}
        onchange={(e) => setSort((e.currentTarget as HTMLSelectElement).value as SeriesSort)}
      >
        <option value="name">Name</option>
        <option value="year">Year</option>
        <option value="added">Added</option>
        <option value="completion">Completion %</option>
      </select>
    </label>
  </div>
  {#if visible.length === 0}
    <p class="text-sm text-slate-500">No series match "{filter}".</p>
  {:else}
    <ul
      bind:this={listEl}
      class="grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5"
    >
      {#each visible as series (series.id)}
        <!-- scroll-margin-top reserves space for the (future, Task 3)
             sticky nav. The CSS variable is declared by the nav when it
             lands; falls back to 0 here so Task 2 works standalone. -->
        <li
          id={`series-${series.id}`}
          style="scroll-margin-top: var(--sticky-nav-height, 0);"
        ><SeriesCard {series} /></li>
      {/each}
    </ul>
    <AlphaScrubber
      items={visible}
      getSortKey={(s: SeriesWithCounts) => s.sort_title}
      getElementId={(s: SeriesWithCounts) => `series-${s.id}`}
      {listEl}
    />
  {/if}
{/if}
