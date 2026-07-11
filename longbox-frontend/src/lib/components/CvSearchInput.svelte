<script lang="ts">
  import { Search } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { searchVolumes } from '$lib/api/cv';
  import type { SeriesSearchResult } from '$lib/types';

  interface Props {
    /** Initial query (e.g. a filename-derived hint). */
    initialQuery?: string;
    onSelect: (result: SeriesSearchResult) => void;
    disabled?: boolean;
  }

  let { initialQuery = '', onSelect, disabled = false }: Props = $props();

  let query = $state(initialQuery);
  let results = $state<SeriesSearchResult[]>([]);
  let filteredCount = $state(0);
  let showFiltered = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let pendingTimer: ReturnType<typeof setTimeout> | null = null;

  // Kick off an initial search if a hint was provided so the user lands
  // on a useful results list without typing. This runs once at component
  // construction; `initialQuery` is a one-shot prop, not reactive.
  if (initialQuery.trim() !== '') schedule(initialQuery);

  function schedule(next: string): void {
    if (pendingTimer !== null) clearTimeout(pendingTimer);
    const trimmed = next.trim();
    if (trimmed === '') {
      results = [];
      filteredCount = 0;
      error = null;
      loading = false;
      return;
    }
    loading = true;
    pendingTimer = setTimeout(() => {
      pendingTimer = null;
      void run(trimmed);
    }, 300);
  }

  async function run(q: string): Promise<void> {
    try {
      const r = await searchVolumes(q, { showFiltered });
      // Drop the result if the query has changed since we kicked off.
      if (q !== query.trim()) return;
      results = r.results;
      // This picker (Change Match / tidy / files) isn't library-aware; it
      // just needs a total to drive the reveal toggle. Sum both hidden
      // buckets — publisher blocklist + already-in-library.
      filteredCount = r.filtered_publisher + r.filtered_in_library;
      error = null;
    } catch (e) {
      results = [];
      filteredCount = 0;
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      if (q === query.trim()) loading = false;
    }
  }

  function toggleShowFiltered(): void {
    showFiltered = !showFiltered;
    const q = query.trim();
    if (q !== '') void run(q);
  }

  function onInput(e: Event): void {
    const v = (e.target as HTMLInputElement).value;
    query = v;
    schedule(v);
  }
</script>

<div>
  <label class="block">
    <span class="mb-1 block text-xs font-medium text-slate-600">Search ComicVine</span>
    <div class="relative">
      <Search
        class="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-slate-400"
        aria-hidden="true"
      />
      <input
        type="search"
        value={query}
        oninput={onInput}
        {disabled}
        placeholder="Series name…"
        class="w-full rounded-md border border-slate-300 py-1.5 pl-8 pr-3 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500 disabled:bg-slate-100"
      />
    </div>
  </label>

  {#if error}
    <p class="mt-2 text-sm text-red-700">{error}</p>
  {/if}

  {#if filteredCount > 0 || showFiltered}
    <label class="mt-2 inline-flex items-center gap-2 text-xs text-slate-600">
      <input
        type="checkbox"
        checked={showFiltered}
        onchange={toggleShowFiltered}
        class="rounded border-slate-300"
      />
      Show filtered results
      {#if filteredCount > 0}
        <span class="text-slate-500">({filteredCount} hidden)</span>
      {/if}
    </label>
  {/if}

  {#if loading}
    <p class="mt-3 text-sm text-slate-500">Searching…</p>
  {:else if results.length > 0}
    <ul class="mt-3 max-h-72 space-y-1 overflow-y-auto rounded-md border border-slate-200">
      {#each results as r (r.cv_id)}
        <li>
          <button
            type="button"
            onclick={() => onSelect(r)}
            {disabled}
            class="flex w-full items-start gap-3 p-2 text-left hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {#if r.cover_url}
              <img
                src={r.cover_url}
                alt=""
                class="size-12 flex-shrink-0 rounded object-cover"
                loading="lazy"
              />
            {:else}
              <div class="size-12 flex-shrink-0 rounded bg-slate-100" aria-hidden="true"></div>
            {/if}
            <div class="min-w-0 flex-1">
              <div class="truncate text-sm font-medium">
                {r.name}{#if r.start_year} <span class="text-slate-500">({r.start_year})</span>{/if}
              </div>
              <div class="truncate text-xs text-slate-500">
                {r.publisher ?? 'Unknown publisher'} · {r.issue_count} issue{r.issue_count === 1 ? '' : 's'}
              </div>
            </div>
          </button>
        </li>
      {/each}
    </ul>
  {:else if query.trim() !== '' && !loading}
    <p class="mt-3 text-sm text-slate-500">No results.</p>
  {/if}
</div>
