<script lang="ts">
  import type { ScanRun } from '$lib/types';
  import { formatDateTime, formatDuration } from '$lib/format';

  interface Props {
    report: ScanRun;
  }

  let { report }: Props = $props();

  // finished_at is null for `running` rows (interrupted-by-restart rows
  // get a finished_at, so this is effectively only mid-scan today).
  const durationMs = $derived.by(() => {
    if (!report.finished_at) return null;
    const start = Date.parse(report.started_at.replace(' ', 'T') + 'Z');
    const end = Date.parse(report.finished_at.replace(' ', 'T') + 'Z');
    if (Number.isNaN(start) || Number.isNaN(end)) return null;
    return end - start;
  });

  const kindLabel = $derived(
    report.kind === 'full'
      ? 'Full scan'
      : report.kind === 'rescan_unmatched'
        ? 'Rescan needs-review'
        : 'Rematch (series)'
  );

  // Walked + New only make sense on full scans. rescan_unmatched and
  // rematch_for_series both route through code paths that never reach
  // the disk-walk counter or the catalog-insert branch, so those fields
  // are structurally always 0. Hiding avoids the "Seen 0 / Added 0
  // looks broken" confusion the Phase B smoke surfaced (per A.7 brief
  // Sub-item C).
  const showWalkedAndNew = $derived(report.kind === 'full');
</script>

<article class="rounded-lg border border-slate-200 bg-white p-4">
  <header class="mb-3 flex items-baseline justify-between gap-3">
    <div class="min-w-0">
      <h3 class="font-semibold">
        {kindLabel}
        <span class="text-xs font-normal text-slate-500">· library root #{report.library_root_id}</span>
      </h3>
      <div class="text-xs text-slate-500">
        {formatDateTime(report.started_at)}{#if durationMs !== null} · {formatDuration(durationMs)}{/if}
      </div>
    </div>
    {#if report.status === 'failed'}
      <span class="rounded-full bg-red-100 px-2 py-0.5 text-xs font-medium text-red-700">
        Failed
      </span>
    {:else if report.status === 'running'}
      <span class="rounded-full bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-800">
        Running
      </span>
    {/if}
  </header>

  <!--
    Three columns of Label: Value pairs. Each pair is one grid item with
    flex internally — the prior `display: contents` wrapper made dt/dd
    flow as twelve independent grid items, which interleaved labels and
    values across rows. Two columns on narrow widths, three on >=sm.
  -->
  <dl class="grid grid-cols-2 gap-x-4 gap-y-1 text-sm sm:grid-cols-3">
    {#if showWalkedAndNew}
      <div class="flex items-baseline gap-1.5">
        <dt class="text-slate-500" title="CBZ files walked during disk traversal">Walked</dt>
        <dd class="font-medium">{report.files_seen}</dd>
      </div>
      <div class="flex items-baseline gap-1.5">
        <dt class="text-slate-500" title="New catalog rows created">New</dt>
        <dd class="font-medium">{report.files_added}</dd>
      </div>
    {/if}
    <div class="flex items-baseline gap-1.5">
      <dt class="text-slate-500" title="Existing catalog rows re-examined">Re-checked</dt>
      <dd class="font-medium">{report.files_updated}</dd>
    </div>
    <div class="flex items-baseline gap-1.5">
      <dt class="text-slate-500">Matched</dt>
      <dd class="font-medium">{report.files_matched}</dd>
    </div>
    <div class="flex items-baseline gap-1.5">
      <dt class="text-slate-500">Needs review</dt>
      <dd class="font-medium">{report.files_needs_review}</dd>
    </div>
    <div class="flex items-baseline gap-1.5">
      <dt class="text-slate-500">Unmatched</dt>
      <dd class="font-medium">{report.files_unmatched}</dd>
    </div>
  </dl>

  {#if report.error_message}
    <div class="mt-3 rounded border-l-2 border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800">
      {report.error_message}
    </div>
  {/if}
</article>
