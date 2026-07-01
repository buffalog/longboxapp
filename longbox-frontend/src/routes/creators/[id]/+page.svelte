<script lang="ts">
  import { getCreatorDiscovery, type DiscoveredVolume } from '$lib/api/creators';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';

  let { data } = $props();

  let discovery = $state<DiscoveredVolume[] | null>(null);
  let discovering = $state(false);
  let discoverError = $state<string | null>(null);
  let addingId = $state<number | null>(null);
  let addedIds = $state<Set<number>>(new Set());

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

  async function acquire(cvVolumeId: number) {
    addingId = cvVolumeId;
    try {
      await addSeries(cvVolumeId);
      addedIds = new Set(addedIds).add(cvVolumeId);
    } finally {
      addingId = null;
    }
  }

  const inLibrary = $derived((discovery ?? []).filter((d) => d.series_id !== null));
  const notInLibrary = $derived((discovery ?? []).filter((d) => d.series_id === null));
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
    <h2 class="mb-2 text-lg font-semibold">Not in your library ({notInLibrary.length})</h2>
    <ul class="mb-6 space-y-1">
      {#each notInLibrary as v (v.cv_volume_id)}
        <li class="flex items-baseline justify-between gap-2">
          <span>{v.name}</span>
          {#if addedIds.has(v.cv_volume_id)}
            <span class="text-sm text-green-600">✓ Added</span>
          {:else}
            <Button
              variant="secondary"
              size="sm"
              loading={addingId === v.cv_volume_id}
              onclick={() => acquire(v.cv_volume_id)}
            >Add to Library</Button>
          {/if}
        </li>
      {/each}
    </ul>

    <h2 class="mb-2 text-lg font-semibold">In your library ({inLibrary.length})</h2>
    <ul class="space-y-1">
      {#each inLibrary as v (v.cv_volume_id)}
        <li><a href={`/series/${v.series_id}`} class="hover:underline">{v.name}</a></li>
      {/each}
    </ul>
  {/if}
</section>
