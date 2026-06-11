<script lang="ts">
  import { BookOpen } from 'lucide-svelte';
  import type { SeriesWithCounts } from '$lib/types';

  interface Props {
    series: SeriesWithCounts;
  }

  let { series }: Props = $props();

  // `available` excludes solicited/pre-release issues — owning everything
  // that has actually shipped is "complete," even if a future issue is
  // already in the catalog. Without this split the badge stays orange for
  // every ongoing series.
  const available = $derived(series.total_count - series.solicited_count);

  const badgeClass = $derived.by(() => {
    if (series.total_count === 0) return 'bg-slate-100 text-slate-600';
    if (available > 0 && series.owned_count >= available)
      return 'bg-status-owned/10 text-status-owned';
    if (series.owned_count === 0) return 'bg-status-unmatched/10 text-status-unmatched';
    return 'bg-status-needs_review/10 text-status-needs_review';
  });
</script>

<a
  href="/series/{series.id}"
  class="flex flex-col overflow-hidden rounded-lg border border-slate-200 bg-white shadow-sm transition hover:shadow"
>
  <div class="aspect-[2/3] bg-slate-100">
    {#if series.cover_url}
      <img
        src={series.cover_url}
        alt=""
        class="size-full object-cover"
        loading="lazy"
      />
    {:else}
      <div class="flex size-full items-center justify-center text-slate-400">
        <BookOpen class="size-12" aria-hidden="true" />
      </div>
    {/if}
  </div>
  <div class="flex flex-col gap-1 p-3">
    <h3 class="line-clamp-2 text-sm font-semibold leading-snug">{series.title}</h3>
    <div class="flex items-center justify-between gap-2 text-xs text-slate-500">
      <span class="truncate">
        {series.start_year ?? '—'}{series.publisher ? ` · ${series.publisher}` : ''}
      </span>
      <span class="flex-shrink-0 rounded-full px-2 py-0.5 text-xs font-medium {badgeClass}">
        {series.owned_count}/{available}{#if series.solicited_count > 0}<span class="text-slate-400"> · +{series.solicited_count}</span>{/if}
      </span>
    </div>
  </div>
</a>
