<script lang="ts">
  import { type DiscoveredVolume } from '$lib/api/creators';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';

  let { volumes }: { volumes: DiscoveredVolume[] } = $props();

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
</script>

{#if volumes.length === 0}
  <p class="text-sm text-slate-500">No series found for this creator.</p>
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
