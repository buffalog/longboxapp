<script lang="ts">
  // Dashboard discovery widget: this ship-week's releases from series
  // the user owns but hasn't subscribed. Self-contained — owns its own
  // "add to pull list" mutation. Renders nothing when there's nothing
  // to surface (a glance widget shouldn't show an empty card).
  import { ApiError } from '$lib/api/client';
  import { addCalendarVolumeToPullList, type ReleaseOfNote } from '$lib/api/releases';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';

  interface Props {
    rows: ReleaseOfNote[];
  }

  let { rows: initialRows }: Props = $props();

  // Capped — a dashboard glance, not a full list (the calendar page is
  // the complete surface).
  const MAX = 6;

  let rows = $state<ReleaseOfNote[]>([...initialRows]);
  let busyVolume = $state<number | null>(null);

  const shown = $derived(rows.slice(0, MAX));

  async function add(row: ReleaseOfNote): Promise<void> {
    busyVolume = row.cv_volume_id;
    try {
      await addCalendarVolumeToPullList(row.cv_volume_id);
      // It's on the pull list now — no longer "of note".
      rows = rows.filter((r) => r.cv_volume_id !== row.cv_volume_id);
      toast.success(`${row.volume_name} added to the pull list.`);
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Could not add to the pull list.');
    } finally {
      busyVolume = null;
    }
  }
</script>

{#if rows.length > 0}
  <section class="mb-6 rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-1 text-base font-semibold">Releases of note</h2>
    <p class="mb-3 text-xs text-slate-500">
      On sale this week from series you own but haven't added to your pull list.
    </p>
    <ul class="space-y-2">
      {#each shown as row (row.cv_volume_id)}
        <li class="flex items-center gap-3">
          {#if row.cover_url}
            <img
              src={row.cover_url}
              alt=""
              class="size-10 flex-shrink-0 rounded bg-slate-100 object-cover"
              loading="lazy"
            />
          {:else}
            <div class="size-10 flex-shrink-0 rounded bg-slate-100" aria-hidden="true"></div>
          {/if}
          <div class="min-w-0 flex-1">
            <a
              href={row.site_detail_url}
              target="_blank"
              rel="noreferrer"
              class="block truncate font-medium text-blue-600 hover:underline"
            >
              {row.volume_name}
            </a>
            <div class="text-xs text-slate-500">
              {row.issue_count} issue{row.issue_count === 1 ? '' : 's'} this week
            </div>
          </div>
          <Button
            size="sm"
            onclick={() => add(row)}
            loading={busyVolume === row.cv_volume_id}
            disabled={busyVolume !== null}
          >
            Add to pull list
          </Button>
        </li>
      {/each}
    </ul>
    {#if rows.length > MAX}
      <p class="mt-2 text-xs text-slate-400">
        and {rows.length - MAX} more this week
      </p>
    {/if}
  </section>
{/if}
