<script lang="ts">
  import { BookOpen } from 'lucide-svelte';
  import type { Snippet } from 'svelte';
  import type { SeriesDetail } from '$lib/types';
  import { htmlToPlainText } from '$lib/text';

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
      {#if actions}
        <div class="flex items-center gap-2">{@render actions()}</div>
      {/if}
    </div>
    <div class="text-sm">
      <span class="rounded bg-slate-100 px-1.5 py-0.5 font-medium">
        {ownedCount}/{totalCount} owned
      </span>
    </div>
    {#if series.description}
      <div class="text-sm text-slate-700">
        {#if descExpanded || !isLongDesc}
          <div class="prose prose-sm max-w-none">{@html series.description}</div>
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
