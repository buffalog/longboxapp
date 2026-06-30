<script lang="ts">
  import { BookOpen } from 'lucide-svelte';
  import type { SeriesWithCounts } from '$lib/types';

  interface Props {
    series: SeriesWithCounts;
  }

  let { series }: Props = $props();

  // Issue-status taxonomy, all derived (nothing is stored on the issue):
  //   Available = owned_count
  //   Solicited = solicited_count       (cover_date in the future; +N, blue)
  //   Released  = available = total - solicited   (issues that should exist)
  //   Missing   = available - owned      (released but not owned; red)
  // `available` is the badge denominator so owning everything *shipped*
  // reads as fine even while a future issue sits in the catalog.
  //
  // Caveat: "solicited" keys off `cover_date`, not the on-sale
  // `store_date` (which isn't persisted per issue). Comic cover dates run
  // ~1–2 months ahead of on-sale, so the solicited/released boundary is
  // approximate. Fixing it means storing store_date — a CV-cache change,
  // out of scope here.
  const available = $derived(series.total_count - series.solicited_count);
  const solicited = $derived(series.solicited_count);
  // `finished` is the Metron Completed|Cancelled signal (GH #7). Always
  // present now (column NOT NULL DEFAULT false); false until enrichment
  // populates it, so the purple branch only fires on an affirmative flag.
  const finished = $derived(series.finished);

  // Colour of the X/Y fraction. `null` when there are no released issues
  // to count (the fraction is suppressed and only +N shows).
  const fractionClass = $derived.by(() => {
    if (available <= 0) return null;
    if (series.owned_count >= available) {
      // Own everything that's shipped. Purple only when the run is
      // confirmed over AND nothing is upcoming — the complete-collection
      // celebration; otherwise neutral (more could still ship).
      return finished && solicited === 0
        ? 'bg-status-finished/10 text-status-finished'
        : 'bg-slate-100 text-slate-600';
    }
    // A released issue is genuinely missing — the only "something's wrong"
    // state. Red regardless of how many (partial or none owned).
    return 'bg-status-unmatched/10 text-status-unmatched';
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
      <span class="flex flex-shrink-0 items-center gap-1">
        {#if available > 0}
          <span
            data-testid="badge-fraction"
            class="rounded-full px-2 py-0.5 text-xs font-medium {fractionClass}"
          >{series.owned_count}/{available}</span>
        {:else if series.total_count === 0}
          <span
            data-testid="badge-empty"
            class="rounded-full bg-slate-100 px-2 py-0.5 text-xs font-medium text-slate-600"
          >0/0</span>
        {/if}
        {#if solicited > 0}
          <span
            data-testid="badge-solicited"
            class="rounded-full bg-status-solicited/10 px-2 py-0.5 text-xs font-medium text-status-solicited"
          >+{solicited}</span>
        {/if}
      </span>
    </div>
  </div>
</a>
