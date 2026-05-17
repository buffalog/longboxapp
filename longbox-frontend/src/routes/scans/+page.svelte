<script lang="ts">
  import { Activity, History } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { triggerFullScan, triggerRescanUnmatched } from '$lib/api/scans';
  import Button from '$lib/components/Button.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import ScanReportCard from '$lib/components/ScanReportCard.svelte';
  import { scanStatus } from '$lib/stores/scanStatus.svelte';
  import { formatRelative } from '$lib/format';

  let { data } = $props();

  let error = $state<ApiError | null>(null);
  let triggeringFull = $state(false);
  let triggeringRescan = $state(false);

  // libraryRoot comes from the layout's load(); see /+layout.ts.
  const libraryRootId = $derived(data.libraryRoot?.id ?? null);

  async function handleFull(): Promise<void> {
    if (libraryRootId === null) return;
    triggeringFull = true;
    error = null;
    try {
      await triggerFullScan(libraryRootId);
      await scanStatus.refresh();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      triggeringFull = false;
    }
  }

  async function handleRescan(): Promise<void> {
    if (libraryRootId === null) return;
    triggeringRescan = true;
    error = null;
    try {
      await triggerRescanUnmatched(libraryRootId);
      await scanStatus.refresh();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      triggeringRescan = false;
    }
  }
</script>

<h1 class="mb-4 text-2xl font-bold">Scans</h1>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

<section class="mb-6 rounded-lg border border-slate-200 bg-white p-4">
  {#if scanStatus.current}
    <div class="flex items-center gap-3">
      <Activity class="size-5 animate-pulse text-blue-600" aria-hidden="true" />
      <div>
        <div class="font-semibold">Scan in progress</div>
        <div class="text-xs text-slate-500">
          {scanStatus.current.kind.replace('_', ' ')} · started {formatRelative(scanStatus.current.started_at)}
        </div>
      </div>
    </div>
  {:else}
    <div class="flex flex-wrap items-center gap-3">
      <span class="text-sm text-slate-600">Trigger a scan:</span>
      <Button onclick={handleFull} loading={triggeringFull}>Full scan</Button>
      <Button variant="secondary" onclick={handleRescan} loading={triggeringRescan}>
        Rescan needs-review files
      </Button>
    </div>
  {/if}
</section>

<h2 class="mb-3 text-lg font-semibold">Recent scans</h2>
{#if scanStatus.recent.length === 0}
  <EmptyState icon={History} title="No scans yet" />
{:else}
  <ul class="space-y-3">
    {#each scanStatus.recent as report (report.started_at + '-' + report.library_root_id)}
      <li><ScanReportCard {report} /></li>
    {/each}
  </ul>
{/if}
