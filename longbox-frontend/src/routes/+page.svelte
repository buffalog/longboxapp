<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError } from '$lib/api/client';
  import { getStats } from '$lib/api/stats';
  import { triggerFullScan } from '$lib/api/scans';
  import { listSeries } from '$lib/api/series';
  import { scanStatus } from '$lib/stores/scanStatus.svelte';
  import { formatRelative } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import LoadingSpinner from '$lib/components/LoadingSpinner.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import type { Stats } from '$lib/types';

  let stats = $state<Stats | null>(null);
  let libraryRootId = $state<number | null>(null);
  let loading = $state(true);
  let error = $state<ApiError | null>(null);
  let triggering = $state(false);

  onMount(async () => {
    try {
      stats = await getStats();
      // Phase A has at most one library root. We discover its id from the
      // most-recent scan report (if any), or by hitting /api/scans/recent
      // which the layout store has already populated. Fallback to a probe
      // via listSeries (cheap) — but really we just need any id; in Phase
      // A there's exactly one and any scan/file uses it.
      if (scanStatus.recent.length > 0) {
        libraryRootId = scanStatus.recent[0]!.library_root_id;
      } else if (scanStatus.current) {
        libraryRootId = scanStatus.current.library_root_id;
      } else {
        // Probe: trigger a 404 scan with id 1 to confirm it exists? Too
        // heavy. Default to 1 for Phase A; bootstrap ensures id 1 exists.
        libraryRootId = 1;
      }
      // Touch listSeries so an empty library shows a useful CTA.
      await listSeries();
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

  const lastScanTime = $derived(scanStatus.recent[0]?.completed_at ?? null);
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
