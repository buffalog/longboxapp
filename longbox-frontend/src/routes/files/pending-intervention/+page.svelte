<script lang="ts">
  import { CircleSlash } from 'lucide-svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import { formatBytes, formatRelative } from '$lib/format';
  import type { InterventionReason } from '$lib/types';

  let { data } = $props();

  // Brief: "Reason (conflict / write-failed / move-failed)" — short
  // human labels for the reason column. Detail strings (the underlying
  // error message) only render for the failure variants since
  // Conflict's reason is self-explanatory.
  function reasonLabel(reason: InterventionReason): string {
    switch (reason.kind) {
      case 'conflict':
        return 'Conflict';
      case 'comic_info_write_failed':
        return 'ComicInfo write failed';
      case 'move_failed':
        return 'Move failed';
    }
  }

  function reasonDetail(reason: InterventionReason): string | null {
    return reason.kind === 'conflict' ? null : reason.detail;
  }
</script>

<header class="mb-4">
  <h1 class="text-2xl font-bold">Pending manual intervention</h1>
  <p class="mt-1 text-sm text-slate-600">
    Files Phase B couldn't process automatically. The source is still in the watch folder; resolve
    each one manually (move, rename, delete the conflicting target, etc.) and the cache clears on
    the next event for that file.
  </p>
</header>

{#if data.pending.count === 0}
  <EmptyState
    icon={CircleSlash}
    title="Nothing pending"
    message="No files are stuck. New conflicts or failures will appear here automatically."
  />
{:else}
  <div class="overflow-x-auto rounded-lg border border-slate-200 bg-white">
    <table class="w-full text-sm">
      <thead class="bg-slate-50 text-left text-xs uppercase text-slate-500">
        <tr>
          <th class="px-3 py-2">Source</th>
          <th class="px-3 py-2">Target</th>
          <th class="px-3 py-2">Reason</th>
          <th class="px-3 py-2 text-right">Size</th>
          <th class="px-3 py-2">Last attempt</th>
        </tr>
      </thead>
      <tbody>
        {#each data.pending.items as item (item.source_path)}
          <tr class="border-t border-slate-100">
            <td class="break-all px-3 py-2 font-mono text-xs">{item.source_path}</td>
            <td class="break-all px-3 py-2 font-mono text-xs text-slate-600">{item.target_path}</td>
            <td class="px-3 py-2">
              <span class="font-medium">{reasonLabel(item.reason)}</span>
              {#if reasonDetail(item.reason)}
                <div class="mt-0.5 text-xs text-slate-500">{reasonDetail(item.reason)}</div>
              {/if}
            </td>
            <td class="whitespace-nowrap px-3 py-2 text-right">{formatBytes(item.size)}</td>
            <td class="whitespace-nowrap px-3 py-2 text-xs text-slate-500">
              {formatRelative(item.last_attempt)}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
