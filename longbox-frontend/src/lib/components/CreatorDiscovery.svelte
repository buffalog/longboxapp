<script lang="ts">
  import { type DiscoveredVolume } from '$lib/api/creators';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';
  import CreatorSeriesCard from '$lib/components/CreatorSeriesCard.svelte';

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

  // Owned volumes are shown by the page's eager "In your library" grid (the
  // only source with issue counts), so discovery only renders the not-owned set.
  const notInLibrary = $derived(volumes.filter((d) => d.series_id === null));

  const gridClass = 'grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5';

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
  <h2 class="mb-2 text-lg font-semibold">Not in your library ({notInLibrary.length})</h2>
  <ul class={gridClass}>
    {#each notInLibrary as v (v.cv_volume_id)}
      <li>
        <CreatorSeriesCard coverUrl={v.cover_url} title={v.name}>
          {#snippet footer()}
            {#if meta(v)}<span class="text-xs text-slate-500">{meta(v)}</span>{/if}
            {#if addedIds.has(v.cv_volume_id)}
              <span class="mt-1 text-sm text-green-600">✓ Added</span>
            {:else}
              <Button
                variant="secondary"
                size="sm"
                class="mt-1 w-full justify-center"
                loading={addingId === v.cv_volume_id}
                onclick={() => acquire(v.cv_volume_id)}>Add to Library</Button
              >
            {/if}
          {/snippet}
        </CreatorSeriesCard>
      </li>
    {/each}
  </ul>

  {#if filteredCount > 0 && onShowFiltered}
    <button class="mt-4 text-sm text-blue-600 hover:underline" onclick={onShowFiltered}>
      {filteredCount}
      {filteredCount === 1 ? 'edition' : 'editions'} hidden by the publisher filter — show
      {filteredCount === 1 ? 'it' : 'them'}
    </button>
  {/if}
{/if}
