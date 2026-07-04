<script lang="ts">
  import { getCreatorDiscovery, type DiscoveryResponse } from '$lib/api/creators';
  import CreatorDiscovery from '$lib/components/CreatorDiscovery.svelte';
  import CreatorSeriesCard from '$lib/components/CreatorSeriesCard.svelte';

  const gridClass = 'grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5';

  let { data } = $props();

  let discovery = $state<DiscoveryResponse | null>(null);
  let discovering = $state(false);
  let discoverError = $state<string | null>(null);
  let showFiltered = $state(false);

  async function loadDiscovery() {
    discovering = true;
    discoverError = null;
    try {
      discovery = await getCreatorDiscovery(data.creator.id, showFiltered);
    } catch (e) {
      discoverError = e instanceof Error ? e.message : 'Failed to load bibliography';
    } finally {
      discovering = false;
    }
  }

  function revealFiltered() {
    showFiltered = true;
    loadDiscovery();
  }
</script>

<h1 class="mb-4 text-2xl font-bold">{data.creator.name}</h1>

<div class="mb-4 flex flex-wrap gap-2">
  {#each data.creator.roles as r (r.role)}
    <span class="rounded-full bg-slate-100 px-3 py-1 text-sm">{r.role} · {r.count}</span>
  {/each}
</div>

{#if data.creator.series.length > 0}
  <h2 class="mb-2 text-lg font-semibold">In your library ({data.creator.series.length})</h2>
  <ul class={gridClass}>
    {#each data.creator.series as s (s.series_id)}
      <li>
        <CreatorSeriesCard coverUrl={s.cover_url} title={s.name} href={`/series/${s.series_id}`}>
          {#snippet footer()}
            <span class="text-xs text-slate-500"
              >{s.issue_count} {s.issue_count === 1 ? 'issue' : 'issues'}</span
            >
          {/snippet}
        </CreatorSeriesCard>
      </li>
    {/each}
  </ul>
{/if}

<section class="mt-8">
  {#if discovery === null}
    <button
      class="rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium hover:bg-slate-50 disabled:opacity-50"
      onclick={loadDiscovery}
      disabled={discovering}
    >
      {discovering ? 'Loading bibliography…' : `Discover more by ${data.creator.name}`}
    </button>
    {#if discoverError}<p class="mt-2 text-sm text-red-600">{discoverError}</p>{/if}
  {:else}
    <CreatorDiscovery
      volumes={discovery.results}
      filteredCount={discovery.filtered_count}
      onShowFiltered={revealFiltered}
    />
  {/if}
</section>
