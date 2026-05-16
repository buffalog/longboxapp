<script lang="ts">
  import { BookOpen } from 'lucide-svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import SeriesCard from '$lib/components/SeriesCard.svelte';
  import Button from '$lib/components/Button.svelte';

  let { data } = $props();

  let filter = $state('');

  const filtered = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return data.series;
    return data.series.filter((s) => s.title.toLowerCase().includes(q));
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
  <div class="mb-4">
    <input
      type="search"
      class="w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      placeholder="Filter by title…"
      bind:value={filter}
    />
  </div>
  {#if filtered.length === 0}
    <p class="text-sm text-slate-500">No series match "{filter}".</p>
  {:else}
    <ul class="grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5">
      {#each filtered as series (series.id)}
        <li><SeriesCard {series} /></li>
      {/each}
    </ul>
  {/if}
{/if}
