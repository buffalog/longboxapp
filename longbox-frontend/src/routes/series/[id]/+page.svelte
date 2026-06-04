<script lang="ts">
  import { goto, invalidateAll } from '$app/navigation';
  import { Edit3, FolderOpen, RefreshCw, Search, Trash2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { setSeriesCvId } from '$lib/api/enrichment';
  import { searchSeriesNow } from '$lib/api/pull';
  import { deleteSeries, getSeriesFolderPath, refreshSeries } from '$lib/api/series';
  import { isSolicited } from '$lib/solicitation';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import CvSearchInput from '$lib/components/CvSearchInput.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import IssueGrid from '$lib/components/IssueGrid.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import PullListToggle from '$lib/components/PullListToggle.svelte';
  import SeriesHeader from '$lib/components/SeriesHeader.svelte';
  import type { SeriesSearchResult } from '$lib/types';

  let { data } = $props();

  let refreshing = $state(false);
  let deleting = $state(false);
  let confirmOpen = $state(false);
  let matchFixerOpen = $state(false);
  let fixingMatch = $state(false);
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
  /// Open the series folder in the host OS's file browser (Finder on
  /// macOS, Explorer on Windows). The backend computes the host path
  /// via the `host_library_path` setting prefix substitution; the
  /// `file://` URL is whatever the host OS will resolve.
  ///
  /// Two real constraints on this affordance:
  ///   1) Without `host_library_path` configured the backend returns
  ///      the container path, which `file://` can't open from the
  ///      host (it points inside Docker). We surface that as a toast
  ///      with a copyable path rather than a broken link.
  ///   2) Even with the host path configured, some browsers (Chrome
  ///      notably) block `file://` from an `http://` origin entirely.
  ///      Safari prompts the user once; Firefox warns. The toast
  ///      fallback keeps the path one Cmd+V away from being useful.
  async function handleShowInFinder(): Promise<void> {
    try {
      const { host_path, host_path_configured } = await getSeriesFolderPath(
        data.series.id
      );
      if (!host_path_configured) {
        await navigator.clipboard.writeText(host_path).catch(() => {});
        toast.warning(
          `host_library_path is not set. Copied container path to clipboard: ${host_path}`
        );
        return;
      }
      // `file://` URLs require absolute paths with proper encoding —
      // spaces and parens in series titles need escaping or the OS
      // either errors or opens the wrong directory.
      const url = `file://${encodeURI(host_path)}`;
      window.open(url, '_blank');
    } catch (e) {
      const message =
        e instanceof ApiError ? e.message : 'Could not look up the folder path.';
      toast.warning(message);
    }
  }

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

  /// PATCH /api/series/:id/cv-id with the picked CV volume. Backend
  /// wipes the old issues, fetches the new CV volume's issues +
  /// metadata, and spawns an auto-rematch. We invalidate the page
  /// load on success so the rebuilt issue list / covers / titles
  /// render with no manual refresh.
  ///
  /// cv_id_in_use is the load-bearing error path: a CV id can only
  /// link to ONE series row, so picking a volume that's already
  /// claimed by another row in the catalog 409s. We surface the
  /// existing series's title in the toast so the user knows where
  /// the duplicate lives — same recovery as Library Tidy's queue
  /// case, just less inline-prescriptive (this surface isn't a
  /// review queue, so we don't render a "delete duplicate" affordance
  /// here).
  async function handlePickCv(result: SeriesSearchResult): Promise<void> {
    if (fixingMatch) return;
    fixingMatch = true;
    error = null;
    try {
      await setSeriesCvId(data.series.id, result.cv_id);
      toast.success(`Linked to ${result.name}.`);
      matchFixerOpen = false;
      await invalidateAll();
    } catch (e) {
      if (e instanceof ApiError && e.code === 'conflict.cv_id_in_use') {
        const d = e.details as { existing_series_title?: string } | null;
        const existing = d?.existing_series_title ?? 'another series';
        toast.warning(`That ComicVine volume is already linked to ${existing}.`);
      } else {
        const message =
          e instanceof ApiError ? e.message : 'Could not link the ComicVine volume.';
        toast.warning(message);
      }
    } finally {
      fixingMatch = false;
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
    <Button
      variant="secondary"
      size="sm"
      onclick={() => (matchFixerOpen = !matchFixerOpen)}
      disabled={fixingMatch}
    >
      <Edit3 class="size-3.5" aria-hidden="true" />
      {data.series.cv_id ? 'Change match' : 'Fix match'}
    </Button>
    <Button variant="secondary" size="sm" onclick={handleShowInFinder}>
      <FolderOpen class="size-3.5" aria-hidden="true" /> Show in Finder
    </Button>
    <PullListToggle seriesId={data.series.id} entry={data.pullEntry} />
  {/snippet}
</SeriesHeader>

{#if matchFixerOpen}
  <!-- Inline CV picker. Pre-populated with the series title so the
       first results land without typing. Picking a volume kicks the
       PATCH /api/series/:id/cv-id flow; on success we invalidate the
       page load so the rebuilt issues + covers + titles render. -->
  <section
    class="mt-4 rounded-lg border border-slate-200 bg-white p-4"
    aria-label="ComicVine match picker"
  >
    <header class="mb-2 flex items-baseline justify-between gap-2">
      <h2 class="text-sm font-semibold">
        {data.series.cv_id ? 'Change ComicVine match' : 'Find ComicVine match'}
      </h2>
      <button
        type="button"
        class="text-xs text-slate-500 hover:text-slate-700"
        onclick={() => (matchFixerOpen = false)}
        disabled={fixingMatch}
      >
        Cancel
      </button>
    </header>
    <p class="mb-3 text-xs text-slate-500">
      Pick a volume to replace this series' identity. The catalog wipes the existing issues, fetches
      the picked volume's issues and metadata from ComicVine, and queues a rematch of files on disk.
    </p>
    <CvSearchInput
      initialQuery={data.series.title}
      onSelect={handlePickCv}
      disabled={fixingMatch}
    />
  </section>
{/if}

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
