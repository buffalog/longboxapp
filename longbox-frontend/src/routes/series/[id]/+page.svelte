<script lang="ts">
  import { goto, invalidateAll } from '$app/navigation';
  import { RefreshCw, Search, Trash2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { searchSeriesNow } from '$lib/api/pull';
  import { deleteSeries, refreshSeries } from '$lib/api/series';
  import { isSolicited } from '$lib/solicitation';
  import { toast } from '$lib/stores/toast.svelte';
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

  // Per-series Search debounce. Same 15 s as IssueRow's per-issue
  // button — the backend's per-series in-flight guard catches
  // duplicate fires silently (returns 409 conflict.pull_search_running
  // for the same series mid-search), but a local debounce keeps
  // rapid double-clicks from generating noise.
  const SEARCH_BUTTON_DISABLED_MS = 15_000;
  let searching = $state(false);

  const ownedCount = $derived(
    data.series.issues.filter((i) => i.file?.status === 'owned').length
  );
  const totalCount = $derived(data.series.issues.length);

  // "Missing" mirrors IssueRow's derivation exactly: no file, and
  // not in the solicited window. Surface the search affordance only
  // when there's at least one such issue — otherwise the button
  // would no-match every call and clutter the header.
  const hasMissingIssues = $derived(
    data.series.issues.some((i) => !i.file && !isSolicited(i.cover_date))
  );

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

  async function handleSearchMissing(): Promise<void> {
    if (searching) return;
    searching = true;
    setTimeout(() => {
      searching = false;
    }, SEARCH_BUTTON_DISABLED_MS);
    try {
      await searchSeriesNow(data.series.id);
      toast.success('Search started.');
    } catch (e) {
      // Real-error surface (not the silent in-flight guard): 404
      // when the series isn't on the pull list, 409 if a search
      // is already running for it, network failures. The endpoint's
      // own error envelope carries the human message — pass it
      // through verbatim rather than rewording.
      const message =
        e instanceof ApiError ? e.message : 'Could not start the search.';
      toast.warning(message);
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
    {#if hasMissingIssues}
      <Button
        variant="secondary"
        size="sm"
        loading={searching}
        disabled={searching}
        onclick={handleSearchMissing}
      >
        <Search class="size-3.5" aria-hidden="true" /> Search missing
      </Button>
    {/if}
    <PullListToggle seriesId={data.series.id} entry={data.pullEntry} />
  {/snippet}
</SeriesHeader>

<section class="mt-6">
  <h2 class="mb-2 text-lg font-semibold">Issues ({totalCount})</h2>
  {#if data.series.issues.length > 0}
    <IssueGrid issues={data.series.issues} seriesId={data.series.id} />
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
