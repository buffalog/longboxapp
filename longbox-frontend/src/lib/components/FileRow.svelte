<script lang="ts">
  import { CheckCircle2, XCircle } from 'lucide-svelte';
  import type { EnrichedFileRow } from '$lib/types';
  import { formatBytes } from '$lib/format';
  import Button from './Button.svelte';
  import ConfidenceMeter from './ConfidenceMeter.svelte';

  /** Brief success overlay + fade-out used by the needs-review triage
   *  workflow. Phase 'success' shows the green checkmark; phase 'fading'
   *  collapses the row. Other status pages don't pass this prop. */
  export type ExitStage = {
    kind: 'matched' | 'ignored';
    phase: 'success' | 'fading';
  };

  interface Props {
    file: EnrichedFileRow;
    onAccept: (file: EnrichedFileRow) => void | Promise<void>;
    onChange: (file: EnrichedFileRow) => void;
    onIgnore: (file: EnrichedFileRow) => void | Promise<void>;
    onClearIgnore?: (file: EnrichedFileRow) => void | Promise<void>;
    busy?: boolean;
    exitStage?: ExitStage | null;
  }

  let {
    file,
    onAccept,
    onChange,
    onIgnore,
    onClearIgnore,
    busy = false,
    exitStage = null
  }: Props = $props();

  const hasMatch = $derived(file.issue !== null && file.series !== null);
  // The "primary" action for keyboard focus jump: Accept Match if a
  // suggestion exists, otherwise Change Match. Tagged via data attribute
  // so the parent can `querySelector` it after a row removal.
  const primaryAction = $derived<'accept' | 'change'>(hasMatch ? 'accept' : 'change');
</script>

<article
  class="relative overflow-hidden rounded-lg border border-slate-200 bg-white p-4 transition-all duration-200 ease-out focus-within:bg-blue-50/30"
  style:max-height={exitStage?.phase === 'fading' ? '0' : '1000px'}
  style:padding={exitStage?.phase === 'fading' ? '0' : undefined}
  style:opacity={exitStage?.phase === 'fading' ? '0' : '1'}
  style:margin-bottom={exitStage?.phase === 'fading' ? '0' : undefined}
  style:border-width={exitStage?.phase === 'fading' ? '0' : undefined}
  aria-hidden={exitStage !== null}
>
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
        data-triage-primary={primaryAction === 'accept' ? 'true' : null}
        data-triage-row-id={file.id}
        onclick={() => onAccept(file)}
      >Accept Match</Button>
      <Button
        variant="secondary"
        size="sm"
        disabled={busy}
        data-triage-primary={primaryAction === 'change' ? 'true' : null}
        data-triage-row-id={file.id}
        onclick={() => onChange(file)}
      >Change Match</Button>
      <Button
        variant="ghost"
        size="sm"
        disabled={busy}
        data-triage-row-id={file.id}
        onclick={() => onIgnore(file)}
      >Mark Ignored</Button>
    {/if}
  </div>

  <!--
    Success overlay during the 300ms 'success' phase. Sits on top of the
    row content; clicks pass through to nothing because we don't want to
    re-trigger the action while it's mid-animation.
  -->
  {#if exitStage?.phase === 'success'}
    <div
      class="pointer-events-none absolute inset-0 flex items-center justify-center gap-2 rounded-lg"
      class:bg-emerald-50={exitStage.kind === 'matched'}
      class:bg-slate-100={exitStage.kind === 'ignored'}
      style:opacity="0.95"
      aria-live="polite"
    >
      {#if exitStage.kind === 'matched'}
        <CheckCircle2 class="size-7 text-emerald-700" aria-hidden="true" />
        <span class="text-base font-semibold text-emerald-900">Matched</span>
      {:else}
        <XCircle class="size-7 text-slate-700" aria-hidden="true" />
        <span class="text-base font-semibold text-slate-800">Ignored</span>
      {/if}
    </div>
  {/if}
</article>
