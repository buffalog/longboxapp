<script lang="ts">
  // Pull-list management view. Self-owned list state, seeded from the
  // load data and maintained from each mutation's response.
  import { ApiError } from '$lib/api/client';
  import {
    checkPull,
    removeFromPullList,
    setPullPaused,
    type PullListEntry
  } from '$lib/api/pull';
  import { formatRelative } from '$lib/format';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';

  let { data } = $props();

  let entries = $state<PullListEntry[]>([...data.entries]);
  let busy = $state(false);
  let error = $state<ApiError | null>(null);

  async function run(fn: () => Promise<void>): Promise<void> {
    busy = true;
    error = null;
    try {
      await fn();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      busy = false;
    }
  }

  function handleTogglePause(entry: PullListEntry): Promise<void> {
    return run(async () => {
      const updated = await setPullPaused(entry.series_id, !entry.paused);
      entries = entries.map((e) =>
        e.series_id === entry.series_id ? { ...e, paused: updated.paused } : e
      );
    });
  }

  function handleRemove(seriesId: number): Promise<void> {
    return run(async () => {
      await removeFromPullList(seriesId);
      entries = entries.filter((e) => e.series_id !== seriesId);
    });
  }

  async function handleCheckNow(): Promise<void> {
    try {
      await checkPull();
      toast.success('Pull sweep started.');
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toast.warning('A pull sweep is already running.');
      } else {
        toast.error(e instanceof ApiError ? e.message : 'Could not start a pull sweep.');
      }
    }
  }
</script>

<div class="mb-1 flex flex-wrap items-baseline justify-between gap-2">
  <h1 class="text-2xl font-bold">Pull list</h1>
  <Button variant="secondary" onclick={handleCheckNow}>Check now</Button>
</div>
<p class="mb-4 text-sm text-slate-600">
  Series subscribed for auto-download. The pull engine sweeps daily; "Check now" forces an
  immediate sweep. Subscribe to a series from its detail page.
</p>

{#if error}
  <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

{#if entries.length === 0}
  <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
    No series on the pull list. Open a series and use "+ Pull list" in its header to subscribe.
  </p>
{:else}
  <div class="overflow-hidden rounded-lg border border-slate-200 bg-white">
    <table class="w-full text-sm">
      <thead class="border-b border-slate-200 bg-slate-50 text-left text-xs text-slate-500">
        <tr>
          <th class="px-4 py-2 font-medium">Series</th>
          <th class="px-4 py-2 font-medium">Status</th>
          <th class="px-4 py-2 font-medium">Last pull</th>
          <th class="px-4 py-2 font-medium">Failures</th>
          <th class="px-4 py-2"></th>
        </tr>
      </thead>
      <tbody class="divide-y divide-slate-100">
        {#each entries as entry (entry.series_id)}
          <tr>
            <td class="px-4 py-2">
              <a href={`/series/${entry.series_id}`} class="font-medium text-blue-600 hover:underline">
                {entry.series_title}
              </a>
              {#if entry.series_start_year}
                <span class="ml-1 text-xs text-slate-500">({entry.series_start_year})</span>
              {/if}
            </td>
            <td class="px-4 py-2">
              {#if entry.paused}
                <span class="rounded bg-amber-50 px-1.5 py-0.5 text-xs font-medium text-amber-700">
                  Paused
                </span>
              {:else}
                <span class="rounded bg-emerald-50 px-1.5 py-0.5 text-xs font-medium text-emerald-700">
                  Active
                </span>
              {/if}
            </td>
            <td class="px-4 py-2 text-slate-600">
              {formatRelative(entry.last_successful_pull_at)}
            </td>
            <td class="px-4 py-2">
              <!-- failure_count is the series-level consecutive-sweep-failure
                   counter — informational, no parking threshold. -->
              {#if entry.failure_count > 0}
                <span class="font-medium text-amber-700">{entry.failure_count}</span>
              {:else}
                <span class="text-slate-400">—</span>
              {/if}
            </td>
            <td class="px-4 py-2">
              <div class="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleTogglePause(entry)}
                  disabled={busy}
                >
                  {entry.paused ? 'Resume' : 'Pause'}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleRemove(entry.series_id)}
                  disabled={busy}
                >
                  Remove
                </Button>
              </div>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
