<script lang="ts">
  import { BookPlus, Search } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { searchVolumes } from '$lib/api/cv';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import LoadingSpinner from '$lib/components/LoadingSpinner.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import type { SeriesSearchResult } from '$lib/types';

  let query = $state('');
  let results = $state<SeriesSearchResult[]>([]);
  let searching = $state(false);
  let addingId = $state<number | null>(null);
  let addedIds = $state<Set<number>>(new Set());
  let error = $state<ApiError | null>(null);
  let successMessage = $state<string | null>(null);

  let debounce: ReturnType<typeof setTimeout> | null = null;

  function handleInput(): void {
    if (debounce) clearTimeout(debounce);
    const q = query.trim();
    if (!q) {
      results = [];
      return;
    }
    debounce = setTimeout(() => {
      void runSearch(q);
    }, 400);
  }

  async function runSearch(q: string): Promise<void> {
    searching = true;
    error = null;
    try {
      results = await searchVolumes(q);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
      results = [];
    } finally {
      searching = false;
    }
  }

  async function handleAdd(cvId: number, name: string): Promise<void> {
    addingId = cvId;
    error = null;
    successMessage = null;
    try {
      await addSeries(cvId);
      addedIds = new Set(addedIds).add(cvId);
      successMessage = `Added "${name}". Issues will sync from ComicVine; the next scan picks them up.`;
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      addingId = null;
    }
  }
</script>

<h1 class="mb-4 text-2xl font-bold">Add series</h1>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}
{#if successMessage}
  <div
    class="mb-4 rounded-md border border-green-300 bg-green-50 p-3 text-sm text-green-800"
    role="status"
  >
    {successMessage}
  </div>
{/if}

<div class="relative mb-4">
  <Search class="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-slate-400" aria-hidden="true" />
  <input
    type="search"
    class="w-full rounded-md border border-slate-300 py-1.5 pl-9 pr-3 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
    placeholder="Search ComicVine…"
    bind:value={query}
    oninput={handleInput}
  />
</div>

{#if searching}
  <LoadingSpinner />
{:else if query.trim() && results.length === 0 && !error}
  <p class="text-sm text-slate-500">No results for "{query}".</p>
{:else if results.length > 0}
  <ul class="grid grid-cols-1 gap-3 sm:grid-cols-2">
    {#each results as r (r.cv_id)}
      <li class="flex gap-3 rounded-lg border border-slate-200 bg-white p-3">
        <div class="size-24 shrink-0 overflow-hidden rounded bg-slate-100">
          {#if r.cover_url}
            <img src={r.cover_url} alt="" class="size-full object-cover" loading="lazy" />
          {/if}
        </div>
        <div class="flex flex-1 flex-col gap-1">
          <div class="text-sm font-semibold">{r.name}</div>
          <div class="text-xs text-slate-500">
            {r.start_year ?? '—'}{r.publisher ? ` · ${r.publisher}` : ''} · {r.issue_count} issues
          </div>
          {#if r.description}
            <p class="line-clamp-2 text-xs text-slate-600">{r.description}</p>
          {/if}
          <div class="mt-1">
            {#if addedIds.has(r.cv_id)}
              <span class="text-xs font-medium text-green-700">✓ Added</span>
            {:else}
              <Button
                size="sm"
                loading={addingId === r.cv_id}
                onclick={() => handleAdd(r.cv_id, r.name)}
              >Add to Library</Button>
            {/if}
          </div>
        </div>
      </li>
    {/each}
  </ul>
{:else if !query.trim()}
  <EmptyState
    icon={BookPlus}
    title="Search ComicVine"
    message="Start typing a series name above to see results. Results stream in after 400ms of inactivity."
  />
{/if}
