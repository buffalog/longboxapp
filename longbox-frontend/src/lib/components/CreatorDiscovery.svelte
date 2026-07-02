<script lang="ts">
  import { type DiscoveredVolume } from '$lib/api/creators';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';

  let {
    volumes,
    filteredCount = 0,
    onShowFiltered,
  }: {
    volumes: DiscoveredVolume[];
    filteredCount?: number;
    onShowFiltered?: () => void;
  } = $props();

  let addingId = $state<number | null>(null);
  let addedIds = $state<Set<number>>(new Set());

  const inLibrary = $derived(volumes.filter((d) => d.series_id !== null));
  const notInLibrary = $derived(volumes.filter((d) => d.series_id === null));

  async function acquire(cvVolumeId: number) {
    addingId = cvVolumeId;
    try {
      await addSeries(cvVolumeId);
      addedIds = new Set(addedIds).add(cvVolumeId);
    } finally {
      addingId = null;
    }
  }

  function meta(v: DiscoveredVolume): string {
    return [v.start_year, v.publisher].filter(Boolean).join(' · ');
  }
</script>

{#if volumes.length === 0}
  <p class="text-sm text-slate-500">No series found for this creator.</p>
{:else}
  <h3 class="mb-2 text-lg font-semibold">Not in your library ({notInLibrary.length})</h3>
  <ul class="mb-2 space-y-2">
    {#each notInLibrary as v (v.cv_volume_id)}
      <li class="flex items-center justify-between gap-3">
        <div class="flex min-w-0 items-center gap-2">
          {#if v.cover_url}
            <img src={v.cover_url} alt={v.name} loading="lazy" class="h-12 w-8 shrink-0 rounded object-cover" />
          {:else}
            <div class="h-12 w-8 shrink-0 rounded bg-slate-100"></div>
          {/if}
          <div class="min-w-0">
            <div class="truncate">{v.name}</div>
            {#if meta(v)}<div class="text-xs text-slate-400">{meta(v)}</div>{/if}
          </div>
        </div>
        {#if addedIds.has(v.cv_volume_id)}
          <span class="shrink-0 text-sm text-green-600">✓ Added</span>
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

  {#if filteredCount > 0 && onShowFiltered}
    <button class="mb-4 text-sm text-blue-600 hover:underline" onclick={onShowFiltered}>
      {filteredCount} {filteredCount === 1 ? 'edition' : 'editions'} hidden by the publisher filter — show {filteredCount === 1 ? 'it' : 'them'}
    </button>
  {/if}

  <h3 class="mb-2 text-lg font-semibold">In your library ({inLibrary.length})</h3>
  <ul class="space-y-1">
    {#each inLibrary as v (v.cv_volume_id)}
      <li><a href={`/series/${v.series_id}`} class="hover:underline">{v.name}</a></li>
    {/each}
  </ul>
{/if}
