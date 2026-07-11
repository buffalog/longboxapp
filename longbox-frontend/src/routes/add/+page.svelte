<script lang="ts">
  import { BookPlus, Search } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { searchVolumes } from '$lib/api/cv';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import LoadingSpinner from '$lib/components/LoadingSpinner.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import type { CvSearchResultItem } from '$lib/api/cv';

  let query = $state('');
  let results = $state<CvSearchResultItem[]>([]);
  let filteredPublisher = $state(0);
  let filteredInLibrary = $state(0);
  let hiddenTotal = $derived(filteredPublisher + filteredInLibrary);
  let showFiltered = $state(false);
  let searching = $state(false);
  let addingId = $state<number | null>(null);
  let addedIds = $state<Set<number>>(new Set());
  let error = $state<ApiError | null>(null);
  let successMessage = $state<string | null>(null);

  // "14 hidden (11 publisher blocklist, 3 already in library)" — each clause
  // dropped when its count is zero, so the two reasons never collapse into
  // one ambiguous number. Empty when nothing is hidden.
  let hiddenLabel = $derived.by(() => {
    const parts: string[] = [];
    if (filteredPublisher > 0) parts.push(`${filteredPublisher} publisher blocklist`);
    if (filteredInLibrary > 0) parts.push(`${filteredInLibrary} already in library`);
    return parts.length > 0 ? `${hiddenTotal} hidden (${parts.join(', ')})` : '';
  });

  let debounce: ReturnType<typeof setTimeout> | null = null;

  function handleInput(): void {
    if (debounce) clearTimeout(debounce);
    const q = query.trim();
    if (!q) {
      results = [];
      filteredPublisher = 0;
      filteredInLibrary = 0;
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
      const r = await searchVolumes(q, { showFiltered });
      results = r.results;
      filteredPublisher = r.filtered_publisher;
      filteredInLibrary = r.filtered_in_library;
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
      results = [];
      filteredPublisher = 0;
      filteredInLibrary = 0;
    } finally {
      searching = false;
    }
  }

  function toggleShowFiltered(): void {
    showFiltered = !showFiltered;
    const q = query.trim();
    if (q !== '') void runSearch(q);
  }

  async function handleAdd(cvId: number, name: string): Promise<void> {
    addingId = cvId;
    error = null;
    successMessage = null;
    try {
      const result = await addSeries(cvId);
      addedIds = new Set(addedIds).add(cvId);
      // Build the success message based on what the auto-search did.
      // `pull_search_queued === 0` covers both (a) no missing-and-shipped
      // issues (rare for a brand-new add) and (b) pull engine not
      // configured (silent skip per spec). Either way, the user gets
      // the plain confirmation; the searching-N variant fires when
      // the engine actually queued work.
      if (result.pull_search_queued > 0) {
        const n = result.pull_search_queued;
        const noun = n === 1 ? 'issue' : 'issues';
        successMessage = `Added "${name}" — searching for ${n} missing ${noun}.`;
      } else {
        successMessage = `Added "${name}". Issues will sync from ComicVine; the next scan picks them up.`;
      }
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

<div class="mb-4">
  <div class="relative">
    <Search class="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-slate-400" aria-hidden="true" />
    <input
      type="search"
      class="w-full rounded-md border border-slate-300 py-1.5 pl-9 pr-3 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      placeholder="Search ComicVine…"
      bind:value={query}
      oninput={handleInput}
    />
  </div>
  {#if hiddenTotal > 0 || showFiltered}
    <label class="mt-2 inline-flex items-center gap-2 text-xs text-slate-600">
      <input
        type="checkbox"
        checked={showFiltered}
        onchange={toggleShowFiltered}
        class="rounded border-slate-300"
      />
      Show filtered results
      {#if hiddenLabel}
        <span class="text-slate-500">{hiddenLabel}</span>
      {/if}
    </label>
  {/if}
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
          <div class="mt-1 flex items-center gap-2">
            {#if r.in_library_series_id != null}
              <!-- Already tracked: no add affordance, link out to the series. -->
              <span
                class="inline-flex items-center rounded bg-blue-50 px-1.5 py-0.5 text-xs font-medium text-blue-700"
              >In Library</span>
              <a
                href={`/series/${r.in_library_series_id}`}
                class="text-xs font-medium text-blue-700 hover:underline"
              >View Series →</a>
            {:else if addedIds.has(r.cv_id)}
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
