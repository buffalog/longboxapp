<script lang="ts">
  import { getCreatorDiscovery, type DiscoveredVolume } from '$lib/api/creators';
  import CreatorDiscovery from '$lib/components/CreatorDiscovery.svelte';

  let { data } = $props();

  let discovery = $state<DiscoveredVolume[] | null>(null);
  let discovering = $state(false);
  let discoverError = $state<string | null>(null);

  async function loadDiscovery() {
    discovering = true;
    discoverError = null;
    try {
      discovery = await getCreatorDiscovery(data.creator.id);
    } catch (e) {
      discoverError = e instanceof Error ? e.message : 'Failed to load bibliography';
    } finally {
      discovering = false;
    }
  }
</script>

<h1 class="mb-4 text-2xl font-bold">{data.creator.name}</h1>

<div class="mb-4 flex flex-wrap gap-2">
  {#each data.creator.roles as r (r.role)}
    <span class="rounded-full bg-slate-100 px-3 py-1 text-sm">{r.role} · {r.count}</span>
  {/each}
</div>

<ul class="space-y-2">
  {#each data.creator.series as s (s.series_id)}
    <li class="flex items-center gap-3">
      <a href={`/series/${s.series_id}`} class="flex items-center gap-3 hover:underline">
        {#if s.cover_url}<img src={s.cover_url} alt="" width="40" class="rounded" />{/if}
        {s.name}
      </a>
      <span class="text-sm text-slate-500">{s.issue_count} issues</span>
    </li>
  {/each}
</ul>

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
    <CreatorDiscovery volumes={discovery} />
  {/if}
</section>
