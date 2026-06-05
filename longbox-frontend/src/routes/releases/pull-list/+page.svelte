<script lang="ts">
  // Pull-list management view. Self-owned list state, seeded from the
  // load data and maintained from each mutation's response.
  import { ApiError } from '$lib/api/client';
  import {
    checkPull,
    exportPullList,
    importPullList,
    removeFromPullList,
    searchSeriesNow,
    setPullPaused,
    type PullListEntry
  } from '$lib/api/pull';
  import { formatRelative } from '$lib/format';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';

  let { data } = $props();

  let entries = $state<PullListEntry[]>([...data.entries]);
  let busy = $state(false);
  let error = $state<ApiError | null>(null);
  // Per-row debounce for the Search-now button. 15 s covers a typical
  // indexer search wall-time without locking the button indefinitely —
  // the backend has no live-status feed for the frontend to poll, so
  // a fixed timer is the pragmatic disabled-state proxy. An impatient
  // re-click after the timer expires still gets a clear 409 toast.
  const SEARCH_BUTTON_DISABLED_MS = 15_000;
  let searchingIds = $state<Set<number>>(new Set());

  async function run(fn: () => Promise<void>): Promise<void> {
    busy = true;
    error = null;
    try {
      await fn();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      busy = false;
    }
  }

  function handleTogglePause(entry: PullListEntry): Promise<void> {
    return run(async () => {
      const updated = await setPullPaused(entry.series_id, !entry.paused);
      entries = entries.map((e) =>
        e.series_id === entry.series_id ? { ...e, paused: updated.paused } : e
      );
    });
  }

  function handleRemove(seriesId: number): Promise<void> {
    return run(async () => {
      await removeFromPullList(seriesId);
      entries = entries.filter((e) => e.series_id !== seriesId);
    });
  }

  async function handleCheckNow(): Promise<void> {
    try {
      await checkPull();
      toast.success('Pull sweep started.');
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toast.warning('A pull sweep is already running.');
      } else {
        toast.error(e instanceof ApiError ? e.message : 'Could not start a pull sweep.');
      }
    }
  }

  /// Download the current pull list as a timestamped JSON file.
  /// Format matches the import endpoint's input contract so a round-
  /// trip restore is just upload-the-file-we-just-downloaded.
  async function handleExport(): Promise<void> {
    try {
      const rows = await exportPullList();
      const blob = new Blob([JSON.stringify(rows, null, 2)], {
        type: 'application/json'
      });
      const ts = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
      const filename = `longbox-pull-list-${ts}.json`;
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      toast.success(`Exported ${rows.length} subscription${rows.length === 1 ? '' : 's'}.`);
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Export failed.');
    }
  }

  let importFileInput = $state<HTMLInputElement | null>(null);

  function triggerImport(): void {
    importFileInput?.click();
  }

  async function handleImportFile(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    try {
      const text = await file.text();
      const parsed: unknown = JSON.parse(text);
      if (!Array.isArray(parsed)) {
        toast.error('Import file must be a JSON array.');
        return;
      }
      // The backend accepts the export row shape; we trim to the
      // fields it reads (cv_id + title for the report). Other fields
      // (series_id, subscribed_at) are intentionally ignored — they
      // aren't portable between LongBox instances.
      const entries = (parsed as Array<Record<string, unknown>>).map((e) => ({
        cv_id: typeof e.cv_id === 'number' ? (e.cv_id as number) : null,
        title: typeof e.title === 'string' ? (e.title as string) : null
      }));
      const summary = await importPullList(entries);
      // Refresh the local list so newly-added subscriptions show
      // without a hard reload.
      const { added, already_subscribed, series_not_found, missing_cv_id } = summary;
      const parts = [];
      if (added > 0) parts.push(`${added} added`);
      if (already_subscribed > 0) parts.push(`${already_subscribed} already subscribed`);
      if (series_not_found > 0) parts.push(`${series_not_found} not in catalog`);
      if (missing_cv_id > 0) parts.push(`${missing_cv_id} skipped (no cv_id)`);
      const message =
        parts.length > 0 ? `Imported: ${parts.join(', ')}.` : 'Imported: no entries.';
      if (added > 0) {
        toast.success(message);
      } else {
        toast.warning(message);
      }
      if (added > 0) {
        // The pull-list page's `data.entries` was loaded by SvelteKit;
        // window.location.reload picks up the new rows on the next
        // render. invalidateAll() would also work but a hard reload
        // avoids any subtle out-of-sync state with the "subscribed
        // ids" Set used elsewhere on this page.
        window.location.reload();
      }
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Import failed.');
    } finally {
      // Clear the input so the same file can be re-selected (browsers
      // suppress the change event if the value doesn't differ).
      input.value = '';
    }
  }

  async function handleSearchNow(entry: PullListEntry): Promise<void> {
    if (searchingIds.has(entry.series_id)) return;
    // Reassign for Svelte 5 $state reactivity on Sets.
    searchingIds = new Set([...searchingIds, entry.series_id]);
    setTimeout(() => {
      const next = new Set(searchingIds);
      next.delete(entry.series_id);
      searchingIds = next;
    }, SEARCH_BUTTON_DISABLED_MS);
    try {
      await searchSeriesNow(entry.series_id);
      toast.success(`Search started for ${entry.series_title}.`);
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toast.warning(`A search is already running for ${entry.series_title}.`);
      } else {
        toast.error(e instanceof ApiError ? e.message : 'Could not start the search.');
      }
    }
  }
</script>

<div class="mb-1 flex flex-wrap items-baseline justify-between gap-2">
  <h1 class="text-2xl font-bold">Pull list</h1>
  <div class="flex gap-2">
    <Button variant="ghost" size="sm" onclick={handleExport}>Export</Button>
    <Button variant="ghost" size="sm" onclick={triggerImport}>Import</Button>
    <Button variant="secondary" onclick={handleCheckNow}>Check now</Button>
  </div>
</div>
<input
  bind:this={importFileInput}
  type="file"
  accept="application/json,.json"
  class="hidden"
  onchange={(e) => void handleImportFile(e)}
/>
<p class="mb-4 text-sm text-slate-600">
  Series subscribed for auto-download. The pull engine sweeps daily; "Check now" forces an
  immediate sweep. Subscribe to a series from its detail page.
</p>

{#if data.pullFailureCount > 0}
  <p class="mb-3 text-sm">
    <a href="/needs-attention" class="font-medium text-amber-700 hover:underline">
      {data.pullFailureCount} pull{data.pullFailureCount === 1 ? '' : 's'} need{data.pullFailureCount ===
      1
        ? 's'
        : ''} attention →
    </a>
  </p>
{/if}

{#if error}
  <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

{#if entries.length === 0}
  <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
    No series on the pull list. Open a series and use "+ Pull list" in its header to subscribe.
  </p>
{:else}
  <div class="overflow-hidden rounded-lg border border-slate-200 bg-white">
    <table class="w-full text-sm">
      <thead class="border-b border-slate-200 bg-slate-50 text-left text-xs text-slate-500">
        <tr>
          <th class="px-4 py-2 font-medium">Series</th>
          <th class="px-4 py-2 font-medium">Status</th>
          <th class="px-4 py-2 font-medium">Last pull</th>
          <th class="px-4 py-2 font-medium">Failures</th>
          <th class="px-4 py-2"></th>
        </tr>
      </thead>
      <tbody class="divide-y divide-slate-100">
        {#each entries as entry (entry.series_id)}
          <tr>
            <td class="px-4 py-2">
              <a href={`/series/${entry.series_id}`} class="font-medium text-blue-600 hover:underline">
                {entry.series_title}
              </a>
              {#if entry.series_start_year}
                <span class="ml-1 text-xs text-slate-500">({entry.series_start_year})</span>
              {/if}
            </td>
            <td class="px-4 py-2">
              {#if entry.paused}
                <span class="rounded bg-amber-50 px-1.5 py-0.5 text-xs font-medium text-amber-700">
                  Paused
                </span>
              {:else}
                <span class="rounded bg-emerald-50 px-1.5 py-0.5 text-xs font-medium text-emerald-700">
                  Active
                </span>
              {/if}
            </td>
            <td class="px-4 py-2 text-slate-600">
              {formatRelative(entry.last_successful_pull_at)}
            </td>
            <td class="px-4 py-2">
              <!-- failure_count is the series-level consecutive-sweep-failure
                   counter — informational, no parking threshold. -->
              {#if entry.failure_count > 0}
                <span class="font-medium text-amber-700">{entry.failure_count}</span>
              {:else}
                <span class="text-slate-400">—</span>
              {/if}
            </td>
            <td class="px-4 py-2">
              <div class="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleSearchNow(entry)}
                  disabled={busy || searchingIds.has(entry.series_id)}
                >
                  Search now
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleTogglePause(entry)}
                  disabled={busy}
                >
                  {entry.paused ? 'Resume' : 'Pause'}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleRemove(entry.series_id)}
                  disabled={busy}
                >
                  Remove
                </Button>
              </div>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
