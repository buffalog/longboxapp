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

  // Folder name the backend will compute and delete. We mirror the
  // backend convention exactly: `{title} ({start_year})` when the
  // year is set, bare `{title}` otherwise. Used purely for the
  // confirmation modal's display — the backend recomputes from the
  // SeriesRow and is the source of truth for the actual rm.
  const seriesFolderName = $derived(
    data.series.start_year
      ? `${data.series.title} (${data.series.start_year})`
      : data.series.title
  );

  // Count of files currently linked to this series (any status) — the
  // honest catalog answer for "how many files am I about to wipe."
  // Files that *live in this folder but aren't linked to this series*
  // (a misassignment we haven't caught yet) would also get nuked by
  // `remove_dir_all`, but we can't show what we don't know.
  const linkedFileCount = $derived(
    data.series.issues.filter((i) => i.file !== null).length
  );

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

  /// Open the destructive-confirm modal when the series has at least
  /// one linked file; otherwise the click goes straight to a DB-only
  /// delete (the "default behavior when zero files on disk" rule from
  /// the spec). We never call `deleteFiles=true` here because there's
  /// nothing on disk to remove — and the backend's path safety guards
  /// still cleanly handle a stray folder if one happens to exist
  /// (folder absent → warn + 200; we don't surface that here).
  async function handleDeleteClick(): Promise<void> {
    if (linkedFileCount === 0) {
      await runDelete(false);
    } else {
      confirmOpen = true;
    }
  }

  async function runDelete(withFiles: boolean): Promise<void> {
    deleting = true;
    error = null;
    try {
      await deleteSeries(data.series.id, withFiles ? { deleteFiles: true } : {});
      await goto('/series');
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
      deleting = false;
      confirmOpen = false;
    }
  }

  async function handleConfirmDelete(): Promise<void> {
    await runDelete(true);
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
  <Button
    variant="danger"
    size="sm"
    loading={deleting && !confirmOpen}
    disabled={deleting}
    onclick={handleDeleteClick}
  >
    <Trash2 class="size-3.5" aria-hidden="true" /> Delete series
  </Button>
</section>

<Modal open={confirmOpen} title="Delete series and files?" onClose={() => (confirmOpen = false)}>
  <p class="text-sm">
    This will permanently delete the series folder and all
    {linkedFileCount} file{linkedFileCount === 1 ? '' : 's'} from disk.
    <strong>This cannot be undone.</strong>
  </p>
  <p class="mt-2 text-sm">
    Folder to be removed:
    <code class="ml-1 break-all rounded bg-slate-100 px-1.5 py-0.5 font-mono text-xs">
      {seriesFolderName}/
    </code>
  </p>
  <p class="mt-2 text-xs text-slate-500">
    If the folder doesn't exist on disk (e.g. it was renamed), the catalog entry is still removed
    and a warning is logged server-side.
  </p>
  <div class="mt-4 flex justify-end gap-2">
    <Button variant="ghost" onclick={() => (confirmOpen = false)} disabled={deleting}>Cancel</Button>
    <Button variant="danger" onclick={handleConfirmDelete} loading={deleting}>
      Delete series and files
    </Button>
  </div>
</Modal>
