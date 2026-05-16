<script lang="ts">
  import type { ScanReport } from '$lib/types';
  import { formatDateTime, formatDuration } from '$lib/format';

  interface Props {
    report: ScanReport;
  }

  let { report }: Props = $props();
  let errorsOpen = $state(false);
</script>

<article class="rounded-lg border border-slate-200 bg-white p-4">
  <header class="mb-3 flex items-baseline justify-between">
    <div>
      <h3 class="font-semibold">Library root #{report.library_root_id}</h3>
      <div class="text-xs text-slate-500">
        {formatDateTime(report.started_at)} · {formatDuration(report.duration_ms)}
      </div>
    </div>
    {#if report.errors.length > 0}
      <span class="rounded-full bg-red-100 px-2 py-0.5 text-xs font-medium text-red-700">
        {report.errors.length} error{report.errors.length === 1 ? '' : 's'}
      </span>
    {/if}
  </header>

  <dl class="grid grid-cols-2 gap-x-4 gap-y-1 text-sm sm:grid-cols-4">
    <div class="contents"><dt class="text-slate-500">Seen</dt><dd>{report.files_seen}</dd></div>
    <div class="contents"><dt class="text-slate-500">Added</dt><dd>{report.files_added}</dd></div>
    <div class="contents"><dt class="text-slate-500">Updated</dt><dd>{report.files_updated}</dd></div>
    <div class="contents"><dt class="text-slate-500">Missing</dt><dd>{report.files_marked_missing}</dd></div>
    <div class="contents"><dt class="text-slate-500">Owned</dt><dd>{report.matched_owned}</dd></div>
    <div class="contents"><dt class="text-slate-500">Needs review</dt><dd>{report.matched_needs_review}</dd></div>
    <div class="contents"><dt class="text-slate-500">Ignored</dt><dd>{report.matched_ignored}</dd></div>
    <div class="contents"><dt class="text-slate-500">Unmatched</dt><dd>{report.unmatched}</dd></div>
  </dl>

  {#if report.errors.length > 0}
    <details bind:open={errorsOpen} class="mt-3 border-t border-slate-100 pt-2">
      <summary class="cursor-pointer text-sm font-medium text-slate-700">
        {errorsOpen ? 'Hide' : 'Show'} errors
      </summary>
      <ul class="mt-2 space-y-1 text-xs">
        {#each report.errors as e (e.path_relative + e.error_message)}
          <li class="rounded bg-slate-50 p-2">
            <div class="font-mono text-slate-700">{e.path_relative || '(no path)'}</div>
            <div class="text-slate-500">{e.error_message}</div>
          </li>
        {/each}
      </ul>
    </details>
  {/if}
</article>
