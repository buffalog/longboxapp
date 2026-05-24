<script lang="ts">
  import { goto, invalidateAll } from '$app/navigation';
  import { RefreshCw, Trash2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { deleteSeries, refreshSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import IssueGrid from '$lib/components/IssueGrid.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import PullListToggle from '$lib/components/PullListToggle.svelte';
  import SeriesHeader from '$lib/components/SeriesHeader.svelte';

  let { data } = $props();

  let refreshing = $state(false);
  let deleting = $state(false);
  let confirmOpen = $state(false);
  let error = $state<ApiError | null>(null);

  const ownedCount = $derived(
    data.series.issues.filter((i) => i.file?.status === 'owned').length
  );
  const totalCount = $derived(data.series.issues.length);

  async function handleRefresh(): Promise<void> {
    refreshing = true;
    error = null;
    try {
      await refreshSeries(data.series.id);
      await invalidateAll();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      refreshing = false;
    }
  }

  async function handleDelete(): Promise<void> {
    deleting = true;
    error = null;
    try {
      await deleteSeries(data.series.id);
      await goto('/series');
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
      deleting = false;
      confirmOpen = false;
    }
  }
</script>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

<SeriesHeader series={data.series} {ownedCount} {totalCount}>
  {#snippet actions()}
    <!-- Refresh re-fetches the volume + issues from ComicVine, which
         requires a cv_id. Shallow series (cv_id NULL — bulk-converted
         folders without a CV link) have nothing to refresh from;
         hide the affordance entirely rather than render a button that
         400s. Matches SeriesHeader/IssueRow's cv-conditional pattern. -->
    {#if data.series.cv_id}
      <Button
        variant="secondary"
        size="sm"
        loading={refreshing}
        onclick={handleRefresh}
      >
        <RefreshCw class="size-3.5" aria-hidden="true" /> Refresh
      </Button>
    {/if}
    <PullListToggle seriesId={data.series.id} entry={data.pullEntry} />
  {/snippet}
</SeriesHeader>

<section class="mt-6">
  <h2 class="mb-2 text-lg font-semibold">Issues ({totalCount})</h2>
  {#if data.series.issues.length > 0}
    <IssueGrid issues={data.series.issues} />
  {:else if data.series.cv_id}
    <p class="text-sm text-slate-500">
      No issues recorded yet for this series. Hit Refresh to fetch from ComicVine.
    </p>
  {:else}
    <p class="text-sm text-slate-500">
      No issues recorded yet. Files in the corresponding folder will appear
      here as the next scan parses and attaches them.
    </p>
  {/if}
</section>

<section class="mt-8 flex justify-end">
  <Button variant="danger" size="sm" onclick={() => (confirmOpen = true)}>
    <Trash2 class="size-3.5" aria-hidden="true" /> Delete series
  </Button>
</section>

<Modal open={confirmOpen} title="Delete series?" onClose={() => (confirmOpen = false)}>
  <p class="text-sm">
    This removes <strong>{data.series.title}</strong> and all its issues from the catalog. Files on
    disk are not touched. Files that were matched to issues in this series will become unmatched on
    the next scan.
  </p>
  <div class="mt-4 flex justify-end gap-2">
    <Button variant="ghost" onclick={() => (confirmOpen = false)} disabled={deleting}>Cancel</Button>
    <Button variant="danger" onclick={handleDelete} loading={deleting}>Delete</Button>
  </div>
</Modal>
