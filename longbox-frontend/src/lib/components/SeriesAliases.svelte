<script lang="ts">
  // Alternate titles auto-pull searches and matches on, for series the
  // indexers post under a longer or different name than ComicVine's.
  import { onMount } from 'svelte';
  import { ApiError } from '$lib/api/client';
  import { getSeriesAliases, setSeriesAliases, type SeriesAliases } from '$lib/api/series';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';

  let { seriesId }: { seriesId: number } = $props();

  let aliases = $state<SeriesAliases | null>(null);
  let draft = $state('');
  let saving = $state(false);

  onMount(async () => {
    try {
      aliases = await getSeriesAliases(seriesId);
      draft = aliases.user.join('\n');
    } catch {
      aliases = null;
    }
  });

  async function save(): Promise<void> {
    saving = true;
    try {
      const user = draft
        .split('\n')
        .map((l) => l.trim())
        .filter((l) => l.length > 0);
      aliases = await setSeriesAliases(seriesId, user);
      draft = aliases.user.join('\n');
      toast.success('Aliases saved');
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Could not save aliases');
    } finally {
      saving = false;
    }
  }
</script>

{#if aliases}
  <section class="mt-6 rounded-lg border border-slate-200 bg-white p-4" aria-label="Search aliases">
    <h2 class="text-sm font-semibold">Search aliases</h2>
    <p class="mb-2 text-xs text-slate-500">
      Other names releases are posted under, one per line. Auto-pull searches each, and takes a
      release only when its title has exactly an alias's words.
    </p>
    {#if aliases.comicvine.length > 0}
      <p class="mb-2 text-xs text-slate-500">
        From ComicVine: {aliases.comicvine.join(', ')}
      </p>
    {/if}
    <label class="sr-only" for="series-aliases">Aliases</label>
    <textarea
      id="series-aliases"
      class="w-full rounded border border-slate-300 p-2 text-sm"
      rows="3"
      placeholder="Teenage Mutant Ninja Turtles: The Last Ronin"
      bind:value={draft}
      disabled={saving}
    ></textarea>
    <div class="mt-2 flex justify-end">
      <Button size="sm" onclick={save} loading={saving}>Save aliases</Button>
    </div>
  </section>
{/if}
