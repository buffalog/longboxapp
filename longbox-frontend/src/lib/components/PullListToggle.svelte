<script lang="ts">
  // Series-detail subscribe control. Sits in the SeriesHeader actions
  // slot. Self-owned state: seeded once from the loaded pull entry,
  // then maintained from each mutation's response.
  import { ApiError } from '$lib/api/client';
  import {
    addToPullList,
    removeFromPullList,
    setPullPaused,
    type PullEntry
  } from '$lib/api/pull';
  import Button from './Button.svelte';

  let { seriesId, entry }: { seriesId: number; entry: PullEntry | null } = $props();

  let current = $state<PullEntry | null>(entry);
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function run(fn: () => Promise<void>): Promise<void> {
    busy = true;
    error = null;
    try {
      await fn();
    } catch (e) {
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  function handleAdd(): Promise<void> {
    return run(async () => {
      current = await addToPullList(seriesId);
    });
  }

  function handleTogglePause(): Promise<void> {
    return run(async () => {
      current = await setPullPaused(seriesId, !(current?.paused ?? false));
    });
  }

  function handleRemove(): Promise<void> {
    return run(async () => {
      await removeFromPullList(seriesId);
      current = null;
    });
  }
</script>

<div class="flex flex-col items-end gap-1">
  {#if current}
    <div class="flex items-center gap-1.5">
      <span
        class="inline-flex items-center rounded px-2 py-1 text-xs font-medium {current.paused
          ? 'bg-amber-50 text-amber-700'
          : 'bg-emerald-50 text-emerald-700'}"
      >
        {current.paused ? 'Pulls paused' : 'On pull list'}
      </span>
      <Button variant="ghost" size="sm" onclick={handleTogglePause} disabled={busy}>
        {current.paused ? 'Resume' : 'Pause'}
      </Button>
      <Button variant="ghost" size="sm" onclick={handleRemove} disabled={busy}>Remove</Button>
    </div>
  {:else}
    <Button variant="secondary" size="sm" onclick={handleAdd} disabled={busy}>
      + Pull list
    </Button>
  {/if}
  {#if error}
    <span class="text-xs text-red-700">{error}</span>
  {/if}
</div>
