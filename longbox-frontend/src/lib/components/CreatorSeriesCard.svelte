<script lang="ts">
  import { BookOpen } from 'lucide-svelte';
  import type { Snippet } from 'svelte';

  // Close variant of SeriesCard: identical card chrome (dimensions, cover
  // treatment, title) but the footer is caller-supplied — the creator page
  // shows issue count for owned series and publisher/year + an Add button for
  // discovery, neither of which fits SeriesCard's owned/total badge. Wrapped in
  // a link when `href` is set (owned → /series/:id), a plain div otherwise
  // (discovery volumes aren't in the library yet).
  //
  // ponytail: duplicates SeriesCard's ~12 lines of chrome rather than
  // refactoring SeriesCard to share it — that would touch the series page and
  // its component test for no gain here. Unify if a third card variant appears.
  let {
    coverUrl,
    title,
    href,
    footer,
  }: {
    coverUrl: string | null;
    title: string;
    href?: string;
    footer?: Snippet;
  } = $props();

  const shell =
    'flex flex-col overflow-hidden rounded-lg border border-slate-200 bg-white shadow-sm';
</script>

{#snippet body()}
  <div class="aspect-[2/3] bg-slate-100">
    {#if coverUrl}
      <img src={coverUrl} alt="" class="size-full object-cover" loading="lazy" />
    {:else}
      <div class="flex size-full items-center justify-center text-slate-400">
        <BookOpen class="size-12" aria-hidden="true" />
      </div>
    {/if}
  </div>
  <div class="flex flex-col gap-1 p-3">
    <h3 class="line-clamp-2 text-sm font-semibold leading-snug">{title}</h3>
    {#if footer}{@render footer()}{/if}
  </div>
{/snippet}

{#if href}
  <a {href} class="{shell} transition hover:shadow">{@render body()}</a>
{:else}
  <div class={shell}>{@render body()}</div>
{/if}
