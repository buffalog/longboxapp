<script lang="ts">
  import { goto, invalidateAll } from '$app/navigation';
  import { Copy, Edit3, RefreshCw, Search, Trash2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { setSeriesCvId } from '$lib/api/enrichment';
  import { searchSeriesNow } from '$lib/api/pull';
  import { deleteSeries, getSeriesFolderPath, refreshSeries } from '$lib/api/series';
  import { isSolicited } from '$lib/solicitation';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import CvSearchInput from '$lib/components/CvSearchInput.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import InteractiveSearch from '$lib/components/InteractiveSearch.svelte';
  import IssueGrid from '$lib/components/IssueGrid.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import PullListToggle from '$lib/components/PullListToggle.svelte';
  import SeriesHeader from '$lib/components/SeriesHeader.svelte';
  import type { IssueWithFile, SeriesSearchResult } from '$lib/types';

  let { data } = $props();

  // Interactive Search modal — one instance, opened per missing-issue row.
  let interactiveOpen = $state(false);
  let interactiveIssue = $state<IssueWithFile | null>(null);

  function openInteractiveSearch(issue: IssueWithFile): void {
    interactiveIssue = issue;
    interactiveOpen = true;
  }

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

  // Authoritative owned+present file count for this series, sourced
  // from the same SQL the backend's delete-series guard uses
  // (`files JOIN issues WHERE series_id=? AND status='owned' AND
  // is_present=1`). The page-load handler embeds it directly in
  // the SeriesDetail response.
  //
  // Defensive fallback to the issues-array derivation when the
  // field is missing. Surfaced as a Safari-specific bug where the
  // field came back undefined on the client even though the API
  // payload included a positive value — likely a Safari disk-cache
  // serving a pre-field response. With the field present (the
  // happy path) we use the authoritative count; with it missing we
  // fall back to whatever the issues array tells us. The fallback
  // underreports on shallow series (which is exactly why the
  // server field exists), but it's the right safety net: a few
  // false positives showing the modal beats a 409 ambush from
  // skipping the modal entirely.
  const ownedFileCount = $derived.by(() => {
    const serverCount = data.series.owned_file_count;
    if (typeof serverCount === 'number') return serverCount;
    return data.series.issues.filter((i) => i.file?.status === 'owned').length;
  });

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
      const { queued, note } = await searchSeriesNow(data.series.id);
      if (queued > 0) {
        toast.success(
          `Searching ${queued} missing issue${queued === 1 ? '' : 's'}…`
        );
      } else {
        toast.info(note ?? 'No missing issues to search.');
      }
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
  /// Path-copy fallback state. Populated only when the Clipboard
  /// API write rejects (insecure context, permission denied, headless
  /// browser without a clipboard backend). The template renders a
  /// readonly input with this path pre-selected so the user can
  /// Cmd+C manually. `null` keeps the fallback panel hidden.
  let copyFallbackPath = $state<string | null>(null);
  let copyFallbackInput = $state<HTMLInputElement | null>(null);
  $effect(() => {
    // Auto-select the path text whenever the fallback appears, so the
    // user only has to press Cmd+C. `select()` is idempotent and a
    // no-op if the element is already focused with text selected.
    if (copyFallbackPath !== null && copyFallbackInput) {
      copyFallbackInput.focus();
      copyFallbackInput.select();
    }
  });

  /// Copy the series folder path to the clipboard. The backend
  /// substitutes the `host_library_path` prefix for the container
  /// `library_root_path` so the copied string is the path the user's
  /// host OS understands — paste into Finder's Cmd+Shift+G, Explorer's
  /// address bar, a terminal `cd`, etc.
  ///
  /// LongBox ships as a Docker container, so the old "Show in Finder"
  /// affordance was structurally broken: neither a backend `open -R`
  /// (Docker can't reach the host shell) nor a frontend `file://` URL
  /// (browsers block `file://` from an `http://` origin) actually
  /// opens Finder. Copy-to-clipboard is the affordance that genuinely
  /// works across host OSes.
  ///
  /// Fallback path when Clipboard API rejects:
  ///   - Insecure context (rare — localhost is considered secure)
  ///   - Permission denied (Safari over LAN without explicit grant)
  ///   - Headless / older browsers without `navigator.clipboard`
  /// In those cases we surface the path in a readonly input below the
  /// header actions, focused and pre-selected, so the user can finish
  /// the copy with a manual Cmd+C.
  async function handleCopyPath(): Promise<void> {
    try {
      const { host_path, host_path_configured } = await getSeriesFolderPath(
        data.series.id
      );
      try {
        await navigator.clipboard.writeText(host_path);
        copyFallbackPath = null;
        if (host_path_configured) {
          toast.success('Path copied — use Cmd+Shift+G in Finder to open.');
        } else {
          // host_library_path unset → backend echoes the container
          // path. Still useful to copy (the operator can paste it
          // into Settings to derive a host translation), but warn
          // so they know it's not Finder-pasteable as-is.
          toast.warning(
            'Container path copied — set HOST_LIBRARY_PATH in Settings for a Finder-pasteable path.'
          );
        }
      } catch {
        // Clipboard API rejected. Reveal the fallback input; the
        // `$effect` above focuses + selects it on the next tick.
        copyFallbackPath = host_path;
        toast.warning(
          'Could not copy automatically. Path selected below — press Cmd+C (or Ctrl+C) to copy.'
        );
      }
    } catch (e) {
      const message =
        e instanceof ApiError ? e.message : 'Could not look up the folder path.';
      toast.warning(message);
    }
  }

  async function handleDeleteClick(): Promise<void> {
    if (ownedFileCount === 0) {
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
    <!-- Primary actions group: refresh/search are the high-traffic
         affordances for an actively-curated series. Refresh requires
         cv_id (shallow series 400 the CV refetch path); Search missing
         requires at least one un-owned issue. If neither condition holds
         the whole group elides so we don't render an empty gap-4 slot. -->
    {#if data.series.cv_id || hasMissingIssues}
      <div class="flex items-center gap-2">
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
      </div>
    {/if}
    <!-- Metadata group: identity + filesystem affordances. Both render
         unconditionally so this group is always present. -->
    <div class="flex items-center gap-2">
      <Button
        variant="secondary"
        size="sm"
        onclick={() => (matchFixerOpen = !matchFixerOpen)}
        disabled={fixingMatch}
      >
        <Edit3 class="size-3.5" aria-hidden="true" />
        {data.series.cv_id ? 'Change match' : 'Fix match'}
      </Button>
      <Button variant="secondary" size="sm" onclick={handleCopyPath}>
        <Copy class="size-3.5" aria-hidden="true" /> Copy Path
      </Button>
    </div>
    <PullListToggle seriesId={data.series.id} entry={data.pullEntry} />
  {/snippet}
</SeriesHeader>

{#if copyFallbackPath !== null}
  <!-- Clipboard write rejected (insecure context, permission denied,
       headless browser). Show the path in a readonly input, focused
       and pre-selected via the `$effect` above so the user only has
       to press Cmd+C. Dismisses on the button — no auto-hide; the
       user may want to keep it visible while switching to Finder. -->
  <section
    class="mt-4 flex items-center gap-2 rounded-lg border border-amber-200 bg-amber-50 p-3"
    aria-label="Folder path copy fallback"
  >
    <input
      bind:this={copyFallbackInput}
      type="text"
      readonly
      value={copyFallbackPath}
      aria-label="Series folder path"
      class="flex-1 rounded-md border border-amber-300 bg-white px-3 py-1.5 font-mono text-xs shadow-sm focus:border-amber-500 focus:outline-none focus:ring-1 focus:ring-amber-500"
      onfocus={(e) => (e.currentTarget as HTMLInputElement).select()}
    />
    <Button variant="ghost" size="sm" onclick={() => (copyFallbackPath = null)}>
      Done
    </Button>
  </section>
{/if}

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
    <IssueGrid
      issues={data.series.issues}
      seriesId={data.series.id}
      onInteractiveSearch={openInteractiveSearch}
    />
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

{#if interactiveIssue}
  <InteractiveSearch
    open={interactiveOpen}
    onClose={() => (interactiveOpen = false)}
    seriesId={data.series.id}
    seriesTitle={data.series.title}
    issueId={interactiveIssue.id}
    issueNumber={interactiveIssue.number}
  />
{/if}

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
    {ownedFileCount} file{ownedFileCount === 1 ? '' : 's'} from disk.
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
