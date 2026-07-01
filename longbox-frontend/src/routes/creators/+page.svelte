<script lang="ts">
  import { searchCreators, type CreatorSearchRow } from '$lib/api/creators';

  let q = $state('');
  let results = $state<CreatorSearchRow[]>([]);
  let timer = $state<ReturnType<typeof setTimeout> | undefined>(undefined);
  let loading = $state(false);

  function onInput() {
    clearTimeout(timer);
    const term = q.trim();
    if (term.length < 2) { results = []; return; }
    timer = setTimeout(async () => {
      loading = true;
      try { results = await searchCreators(term); }
      finally { loading = false; }
    }, 300);
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
<ul class="space-y-1">
  {#each results as c (c.id)}
    <li class="flex items-baseline gap-2">
      <a href={`/creators/${c.id}`} class="font-medium hover:underline">{c.name}</a>
      <span class="text-sm text-slate-500">{c.issue_count} issues · {c.series_count} series</span>
    </li>
  {/each}
</ul>
