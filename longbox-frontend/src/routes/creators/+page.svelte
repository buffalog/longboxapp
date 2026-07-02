<script lang="ts">
  import {
    searchCreators, searchCvCreators, discoverByCvPerson,
    type CreatorSearchRow, type CvCreatorCandidate, type DiscoveredVolume,
  } from '$lib/api/creators';
  import CreatorDiscovery from '$lib/components/CreatorDiscovery.svelte';

  let q = $state('');
  let results = $state<CreatorSearchRow[]>([]);
  let cvResults = $state<CvCreatorCandidate[]>([]);
  let timer = $state<ReturnType<typeof setTimeout> | undefined>(undefined);
  let loading = $state(false);

  let expanded = $state<number | null>(null);
  let expandedVolumes = $state<DiscoveredVolume[] | null>(null);
  let expandLoading = $state(false);

  function onInput() {
    clearTimeout(timer);
    const term = q.trim();
    expanded = null; expandedVolumes = null;
    if (term.length < 2) { results = []; cvResults = []; return; }
    timer = setTimeout(async () => {
      loading = true;
      try {
        [results, cvResults] = await Promise.all([
          searchCreators(term),
          searchCvCreators(term).catch(() => []),
        ]);
      } finally { loading = false; }
    }, 300);
  }

  async function toggleDiscover(cvPersonId: number) {
    if (expanded === cvPersonId) { expanded = null; expandedVolumes = null; return; }
    expanded = cvPersonId;
    expandedVolumes = null;
    expandLoading = true;
    try {
      expandedVolumes = await discoverByCvPerson(cvPersonId);
    } finally {
      expandLoading = false;
    }
  }
</script>

<h1 class="mb-4 text-2xl font-bold">Creators</h1>
<input
  type="search"
  class="mb-4 w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
  placeholder="Search creators…"
  bind:value={q}
  oninput={onInput}
/>
{#if loading}<p class="text-sm text-slate-500">Searching…</p>{/if}

{#if results.length > 0}
  <h2 class="mb-1 text-sm font-semibold text-slate-600">In your library</h2>
  <ul class="space-y-1">
    {#each results as c (c.id)}
      <li class="flex items-baseline gap-2">
        <a href={`/creators/${c.id}`} class="font-medium hover:underline">{c.name}</a>
        <span class="text-sm text-slate-500">{c.issue_count} issues · {c.series_count} series</span>
      </li>
    {/each}
  </ul>
{/if}

{#if cvResults.length > 0}
  <h2 class="mb-1 mt-5 text-sm font-semibold text-slate-600">Not in your library · ComicVine</h2>
  <ul class="space-y-3">
    {#each cvResults as p (p.cv_person_id)}
      <li class="rounded-md border border-slate-200 p-3">
        <div class="flex items-start gap-3">
          {#if p.image_url}
            <img src={p.image_url} alt={p.name} class="h-12 w-12 shrink-0 rounded object-cover" />
          {/if}
          <div class="min-w-0 flex-1">
            <div class="flex items-baseline gap-2">
              <span class="font-medium">{p.name}</span>
              {#if p.country}<span class="text-xs text-slate-400">{p.country}</span>{/if}
            </div>
            {#if p.description}<p class="text-sm text-slate-500">{p.description}</p>{/if}
            <div class="mt-1">
              {#if p.in_library_creator_id !== null}
                <a href={`/creators/${p.in_library_creator_id}`} class="text-sm text-blue-600 hover:underline">already in your library ↗</a>
              {:else}
                <button class="text-sm text-blue-600 hover:underline" aria-expanded={expanded === p.cv_person_id} onclick={() => toggleDiscover(p.cv_person_id)}>{expanded === p.cv_person_id ? 'Hide' : 'Discover ▾'}</button>
              {/if}
            </div>
            {#if expanded === p.cv_person_id}
              {#if expandLoading}
                <p class="mt-2 text-sm text-slate-500">Loading bibliography…</p>
              {:else if expandedVolumes !== null}
                <CreatorDiscovery volumes={expandedVolumes} />
              {/if}
            {/if}
          </div>
        </div>
      </li>
    {/each}
  </ul>
{/if}

{#if !loading && q.trim().length >= 2 && results.length === 0 && cvResults.length === 0}
  <p class="text-sm text-slate-500">No creators found.</p>
{/if}
