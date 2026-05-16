<script lang="ts">
  import type { EnrichedFileRow } from '$lib/types';
  import { formatBytes } from '$lib/format';
  import Button from './Button.svelte';
  import ConfidenceMeter from './ConfidenceMeter.svelte';

  interface Props {
    file: EnrichedFileRow;
    onAccept: (file: EnrichedFileRow) => void | Promise<void>;
    onChange: (file: EnrichedFileRow) => void;
    onIgnore: (file: EnrichedFileRow) => void | Promise<void>;
    onClearIgnore?: (file: EnrichedFileRow) => void | Promise<void>;
    busy?: boolean;
  }

  let { file, onAccept, onChange, onIgnore, onClearIgnore, busy = false }: Props = $props();

  const hasMatch = $derived(file.issue !== null && file.series !== null);
</script>

<article class="rounded-lg border border-slate-200 bg-white p-4">
  <header class="mb-2 flex items-start justify-between gap-3">
    <div class="min-w-0 flex-1">
      <div class="truncate font-mono text-sm text-slate-700">{file.path_relative}</div>
      <div class="text-xs text-slate-500">{formatBytes(file.size_bytes)}</div>
    </div>
    <span
      class="rounded-full bg-slate-100 px-2 py-0.5 text-xs font-medium uppercase tracking-wide text-slate-600"
    >
      {file.status}
    </span>
  </header>

  {#if hasMatch}
    <div class="mb-3 rounded-md border border-slate-200 bg-slate-50 p-2.5">
      <div class="text-sm font-medium">
        {file.series?.title}
        {#if file.series?.start_year}
          <span class="text-slate-500">({file.series.start_year})</span>
        {/if}
        <span class="text-slate-400">·</span>
        <span class="font-mono">#{file.issue?.number}</span>
        {#if file.issue?.title}
          <span class="text-slate-500"> {file.issue.title}</span>
        {/if}
      </div>
      <div class="mt-1">
        <ConfidenceMeter confidence={file.match_confidence} method={file.match_method} />
      </div>
    </div>
  {:else}
    <div class="mb-3 text-xs italic text-slate-500">
      No suggested match. Use "Change Match" to pick one, or "Mark Ignored".
    </div>
  {/if}

  <div class="flex flex-wrap gap-2">
    {#if file.status === 'ignored'}
      <Button
        variant="secondary"
        size="sm"
        disabled={busy}
        onclick={() => onClearIgnore?.(file)}
      >Restore</Button>
    {:else}
      <Button
        variant="primary"
        size="sm"
        disabled={!hasMatch || busy}
        onclick={() => onAccept(file)}
      >Accept Match</Button>
      <Button
        variant="secondary"
        size="sm"
        disabled={busy}
        onclick={() => onChange(file)}
      >Change Match</Button>
      <Button
        variant="ghost"
        size="sm"
        disabled={busy}
        onclick={() => onIgnore(file)}
      >Mark Ignored</Button>
    {/if}
  </div>
</article>
