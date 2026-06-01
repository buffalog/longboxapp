<script lang="ts">
  // Dashboard discovery widget: this ship-week's releases from series
  // the user owns but hasn't subscribed. Self-contained — owns its own
  // "add to pull list" mutations (single + bulk). Renders nothing when
  // there's nothing to surface (a glance widget shouldn't show an empty
  // card).
  //
  // A.9 Step 4 — added rows are NOT removed; they stay in place with an
  // "On pull list" badge (the calendar's emerald pill) so the user sees
  // their action land. Bulk selection mirrors the calendar.
  import { ApiError } from '$lib/api/client';
  import {
    addCalendarVolumeToPullList,
    bulkAddCalendarVolumesToPullList,
    type ReleaseOfNote
  } from '$lib/api/releases';
  import { createSelection } from '$lib/createSelection.svelte';
  import { toast } from '$lib/stores/toast.svelte';
  import BulkActionBar from '$lib/components/BulkActionBar.svelte';
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
  let bulkBusy = $state(false);
  // cv_volume_ids added to the pull list this session — these rows swap
  // their Add button for the "On pull list" badge in place.
  let added = $state<Set<number>>(new Set());

  const sel = createSelection<number>();

  const shown = $derived(rows.slice(0, MAX));
  // Shown rows not yet added — the bulk action's working set.
  const selectableVolumeIds = $derived(
    shown.filter((r) => !added.has(r.cv_volume_id)).map((r) => r.cv_volume_id)
  );
  const anyBusy = $derived(bulkBusy || busyVolume !== null);

  async function add(row: ReleaseOfNote): Promise<void> {
    busyVolume = row.cv_volume_id;
    try {
      await addCalendarVolumeToPullList({ cv_volume_id: row.cv_volume_id });
      added = new Set(added).add(row.cv_volume_id);
      toast.success(`${row.volume_name} added to the pull list.`);
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Could not add to the pull list.');
    } finally {
      busyVolume = null;
    }
  }

  async function bulkAdd(): Promise<void> {
    const ids = [...sel.selected];
    if (ids.length === 0) return;
    bulkBusy = true;
    try {
      const items = ids.map((id) => ({ cv_volume_id: id }));
      const { results } = await bulkAddCalendarVolumesToPullList(items);
      const addedN = results.filter((r) => r.status === 'added').length;
      const already = results.filter((r) => r.status === 'already_on_list').length;
      const failed = results.filter((r) => r.status === 'failed').length;
      const next = new Set(added);
      for (const r of results) {
        if (r.status === 'failed') continue;
        if (r.cv_volume_id != null) next.add(r.cv_volume_id);
      }
      added = next;
      sel.clear();
      const parts = [`${addedN} added`];
      if (already > 0) parts.push(`${already} already on pull list`);
      if (failed > 0) parts.push(`${failed} failed`);
      const msg = `${parts.join(', ')}.`;
      if (failed > 0) toast.warning(msg);
      else toast.success(msg);
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Bulk add failed.');
    } finally {
      bulkBusy = false;
    }
  }
</script>

{#if rows.length > 0}
  <section class="mb-6 rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-1 text-base font-semibold">Releases of note</h2>
    <p class="mb-3 text-xs text-slate-500">
      On sale this week from series you own but haven't added to your pull list.
    </p>
    {#if selectableVolumeIds.length > 0}
      <BulkActionBar
        count={sel.size}
        allSelected={sel.allSelected(selectableVolumeIds)}
        someSelected={sel.someSelected(selectableVolumeIds)}
        onToggleAll={() => sel.toggleAll(selectableVolumeIds)}
        selectAllLabel="Select all releases of note"
      >
        {#snippet action()}
          <Button
            size="sm"
            onclick={bulkAdd}
            loading={bulkBusy}
            disabled={anyBusy || sel.size === 0}
          >
            {sel.size > 0
              ? `Add ${sel.size} selected to pull list`
              : 'Add selected to pull list'}
          </Button>
        {/snippet}
      </BulkActionBar>
    {/if}
    <ul class="space-y-2">
      {#each shown as row (row.cv_volume_id)}
        <li class="flex items-center gap-3">
          {#if added.has(row.cv_volume_id)}
            <!-- Spacer keeps the row aligned with checkboxed peers. -->
            <div class="w-4 flex-shrink-0" aria-hidden="true"></div>
          {:else}
            <input
              type="checkbox"
              class="flex-shrink-0 rounded border-slate-300"
              checked={sel.has(row.cv_volume_id)}
              onchange={() => sel.toggle(row.cv_volume_id)}
              disabled={anyBusy}
              aria-label={`Select ${row.volume_name}`}
            />
          {/if}
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
          {#if added.has(row.cv_volume_id)}
            <span
              class="rounded bg-emerald-50 px-1.5 py-0.5 text-xs font-medium text-emerald-700"
            >
              On pull list
            </span>
          {:else}
            <Button
              size="sm"
              onclick={() => add(row)}
              loading={busyVolume === row.cv_volume_id}
              disabled={anyBusy}
            >
              Add to pull list
            </Button>
          {/if}
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
