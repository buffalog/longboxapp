<script lang="ts">
  // One file's raw evidence, laid out so the four independent identity claims
  // sit side by side and disagreement is visible at a glance:
  //   where it is bound  |  what its filename says  |  what its archive says
  //   |  what its ComicInfo says
  //
  // No suggested keeper, no ranking, no "we think this one". Interpretation
  // ships with the actions it drives.
  import { Layers } from 'lucide-svelte';
  import { formatBytes } from '$lib/format';
  import type { FileEvidence } from '$lib/api/integrity';

  interface Props {
    file: FileEvidence;
    /** How many finding classes this file appears in, when more than one. */
    classes?: number;
  }
  let { file, classes }: Props = $props();

  // A claim disagrees with the binding it sits next to. Highlighted rather
  // than resolved — which source is right is a judgement the user makes.
  const norm = (s: string | null) => (s ?? '').replace(/^0+/, '').toLowerCase() || null;
  const bound = $derived(norm(file.issue_number));
  const fnDisagrees = $derived(!!file.filename_issue && norm(file.filename_issue) !== bound);
  const arcDisagrees = $derived(!!file.archive_issue && norm(file.archive_issue) !== bound);
  const ciDisagrees = $derived(!!file.comicinfo_number && norm(file.comicinfo_number) !== bound);
</script>

<li class="px-3 py-2 text-sm">
  <div class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
    <span class="font-mono text-xs break-all text-slate-700">{file.path_relative}</span>
    <span class="text-xs text-slate-500">{formatBytes(file.size_bytes)}</span>
    {#if classes && classes > 1}
      <span
        class="inline-flex items-center gap-1 rounded bg-amber-50 px-1.5 py-0.5 text-xs font-medium text-amber-800"
        title="Two independent checks both flagged this file"
      >
        <Layers class="size-3" aria-hidden="true" /> in {classes} checks
      </span>
    {/if}
  </div>

  <div class="mt-1 grid gap-x-4 gap-y-0.5 text-xs sm:grid-cols-2 lg:grid-cols-4">
    <div>
      <span class="text-slate-400">bound to</span>
      <span class="text-slate-700">
        {#if file.series_title}
          {file.series_title}{#if file.series_start_year}&nbsp;({file.series_start_year}){/if}
          #{file.issue_number ?? '?'}
        {:else}
          <span class="text-amber-800">nothing</span>
        {/if}
      </span>
      <span class="text-slate-400"> · {file.match_method} {file.match_confidence.toFixed(2)}</span>
    </div>
    <div class={fnDisagrees ? 'text-amber-800' : 'text-slate-500'}>
      <span class="text-slate-400">filename says</span>
      {file.filename_issue ?? '—'}
    </div>
    <div class={arcDisagrees ? 'text-amber-800' : 'text-slate-500'}>
      <span class="text-slate-400">archive says</span>
      {#if file.archive_issue || file.archive_series}
        {file.archive_series ?? ''}{#if file.archive_issue}&nbsp;#{file.archive_issue}{/if}
      {:else}
        —
      {/if}
    </div>
    <div class={ciDisagrees ? 'text-amber-800' : 'text-slate-500'}>
      <span class="text-slate-400">ComicInfo says</span>
      {#if file.comicinfo_number || file.comicinfo_series}
        {file.comicinfo_series ?? ''}{#if file.comicinfo_number}&nbsp;#{file.comicinfo_number}{/if}
      {:else}
        —
      {/if}
    </div>
  </div>
</li>
