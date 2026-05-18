<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError } from '$lib/api/client';
  import { getStats } from '$lib/api/stats';
  import { triggerFullScan } from '$lib/api/scans';
  import { scanStatus } from '$lib/stores/scanStatus.svelte';
  import { formatRelative } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import LoadingSpinner from '$lib/components/LoadingSpinner.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import type { Stats } from '$lib/types';

  let { data } = $props();

  let stats = $state<Stats | null>(null);
  let loading = $state(true);
  let error = $state<ApiError | null>(null);
  let triggering = $state(false);

  // libraryRoot comes from the layout's load(). It's null only if
  // /api/library-roots failed at boot — which would have also surfaced
  // via the global ErrorBanner.
  const libraryRootId = $derived(data.libraryRoot?.id ?? null);

  onMount(async () => {
    try {
      stats = await getStats();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      loading = false;
    }
  });

  async function triggerScan(): Promise<void> {
    if (libraryRootId === null) return;
    triggering = true;
    error = null;
    try {
      await triggerFullScan(libraryRootId);
      await scanStatus.refresh();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      triggering = false;
    }
  }

  // "Last scan" on the dashboard means: most recent successfully-completed
  // scan that did real work. A no-op rescan-unmatched against a zero-
  // needs-review catalog still gets a `completed` row with files_seen=0;
  // surfacing it as "Last scan completed Xs ago" misrepresents activity.
  // Full scans always count even if they processed nothing, since the
  // disk walk itself is the work.
  const lastScanTime = $derived(
    scanStatus.recent.find(
      (r) => r.status === 'completed' && (r.files_seen > 0 || r.kind === 'full')
    )?.finished_at ?? null
  );
</script>

<h1 class="mb-4 text-2xl font-bold">Dashboard</h1>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

{#if loading}
  <LoadingSpinner />
{:else if stats}
  <section class="mb-6 grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
    <div class="rounded-lg border border-slate-200 bg-white p-3">
      <div class="text-xs uppercase text-slate-500">Series</div>
      <div class="text-2xl font-semibold">{stats.total_series}</div>
    </div>
    <div class="rounded-lg border border-slate-200 bg-white p-3">
      <div class="text-xs uppercase text-slate-500">Issues</div>
      <div class="text-2xl font-semibold">{stats.total_issues}</div>
    </div>
    <div class="rounded-lg border border-slate-200 bg-white p-3">
      <div class="text-xs uppercase text-status-owned">Owned</div>
      <div class="text-2xl font-semibold">{stats.owned_files}</div>
    </div>
    <div class="rounded-lg border border-slate-200 bg-white p-3">
      <div class="text-xs uppercase text-status-needs_review">Needs review</div>
      <div class="text-2xl font-semibold">{stats.needs_review_files}</div>
    </div>
    <div class="rounded-lg border border-slate-200 bg-white p-3">
      <div class="text-xs uppercase text-status-unmatched">Unmatched</div>
      <div class="text-2xl font-semibold">{stats.unmatched_files}</div>
    </div>
    <div class="rounded-lg border border-slate-200 bg-white p-3">
      <div class="text-xs uppercase text-status-missing">Missing</div>
      <div class="text-2xl font-semibold">{stats.missing_issues}</div>
    </div>
  </section>

  <section class="mb-6 rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">Scan status</h2>
    {#if scanStatus.current}
      <p class="text-sm">
        <strong>Scanning…</strong> Started {formatRelative(scanStatus.current.started_at)} ·
        <a class="text-blue-600 hover:underline" href="/scans">details</a>
      </p>
    {:else if lastScanTime}
      <p class="text-sm">
        Last scan completed {formatRelative(lastScanTime)} ·
        <a class="text-blue-600 hover:underline" href="/scans">history</a>
      </p>
    {:else}
      <p class="text-sm text-slate-600">No scans yet. Run one to populate the catalog.</p>
    {/if}
  </section>

  <section class="flex flex-wrap gap-3">
    <Button onclick={() => (window.location.href = '/add')}>Add series</Button>
    <Button
      variant="secondary"
      onclick={triggerScan}
      loading={triggering}
      disabled={!!scanStatus.current || libraryRootId === null}
    >
      {scanStatus.current ? 'Scanning…' : 'Scan library'}
    </Button>
  </section>
{/if}
