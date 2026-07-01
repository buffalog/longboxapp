<script lang="ts">
  let { data } = $props();
</script>

<h1 class="mb-4 text-2xl font-bold">{data.creator.name}</h1>

<div class="mb-4 flex flex-wrap gap-2">
  {#each data.creator.roles as r (r.role)}
    <span class="rounded-full bg-slate-100 px-3 py-1 text-sm">{r.role} · {r.count}</span>
  {/each}
</div>

<ul class="space-y-2">
  {#each data.creator.series as s (s.series_id)}
    <li class="flex items-center gap-3">
      <a href={`/series/${s.series_id}`} class="flex items-center gap-3 hover:underline">
        {#if s.cover_url}<img src={s.cover_url} alt="" width="40" class="rounded" />{/if}
        {s.name}
      </a>
      <span class="text-sm text-slate-500">{s.issue_count} issues</span>
    </li>
  {/each}
</ul>
