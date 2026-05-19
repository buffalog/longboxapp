<script lang="ts">
  import { BookOpen, ExternalLink } from 'lucide-svelte';
  import type { Snippet } from 'svelte';
  import type { SeriesDetail } from '$lib/types';
  import { absolutizeCvLinks, htmlToPlainText } from '$lib/text';
  import { cvSeriesUrl } from '$lib/format';

  interface Props {
    series: SeriesDetail;
    ownedCount: number;
    totalCount: number;
    actions?: Snippet;
  }

  let { series, ownedCount, totalCount, actions }: Props = $props();
  let descExpanded = $state(false);
  const isLongDesc = $derived((series.description?.length ?? 0) > 240);
  const collapsedPreview = $derived(
    series.description ? htmlToPlainText(series.description) : ''
  );
  // CV's description HTML uses path-only hrefs that would otherwise
  // resolve against LongBox's origin. Series-level CV link in the
  // header (below) is the deliberate, always-present affordance; the
  // description's embedded links remain absolutized but aren't a
  // design surface we control.
  const expandedHtml = $derived(
    series.description ? absolutizeCvLinks(series.description) : ''
  );
  const cvUrl = $derived(series.cv_id ? cvSeriesUrl(series.cv_id) : null);
</script>

<header class="flex flex-col gap-4 rounded-lg border border-slate-200 bg-white p-5 sm:flex-row">
  <div class="size-32 shrink-0 overflow-hidden rounded-md bg-slate-100 sm:size-48">
    {#if series.cover_url}
      <img src={series.cover_url} alt="" class="size-full object-cover" />
    {:else}
      <div class="flex size-full items-center justify-center text-slate-400">
        <BookOpen class="size-12" aria-hidden="true" />
      </div>
    {/if}
  </div>
  <div class="flex flex-1 flex-col gap-2">
    <div class="flex flex-wrap items-start gap-3">
      <div class="flex-1">
        <h1 class="text-xl font-bold leading-tight">{series.title}</h1>
        <div class="text-sm text-slate-600">
          {series.start_year ?? '—'}{series.publisher ? ` · ${series.publisher}` : ''}
        </div>
      </div>
      <div class="flex items-center gap-2">
        {#if cvUrl}
          <a
            href={cvUrl}
            target="_blank"
            rel="noopener noreferrer"
            class="inline-flex items-center gap-1 text-sm font-medium text-blue-600 hover:underline"
          >View on ComicVine <ExternalLink class="size-3.5" aria-hidden="true" /></a>
        {/if}
        {#if actions}
          {@render actions()}
        {/if}
      </div>
    </div>
    <div class="text-sm">
      <span class="rounded bg-slate-100 px-1.5 py-0.5 font-medium">
        {ownedCount}/{totalCount} owned
      </span>
    </div>
    {#if series.description}
      <div class="text-sm text-slate-700">
        {#if descExpanded || !isLongDesc}
          <div class="prose prose-sm max-w-none">{@html expandedHtml}</div>
          {#if isLongDesc}
            <button
              type="button"
              class="mt-1 text-xs font-medium text-blue-600 hover:underline"
              onclick={() => (descExpanded = false)}
            >Show less</button>
          {/if}
        {:else}
          <p class="line-clamp-3 whitespace-pre-line">{collapsedPreview}</p>
          <button
            type="button"
            class="mt-1 text-xs font-medium text-blue-600 hover:underline"
            onclick={() => (descExpanded = true)}
          >Show more</button>
        {/if}
      </div>
    {/if}
  </div>
</header>
