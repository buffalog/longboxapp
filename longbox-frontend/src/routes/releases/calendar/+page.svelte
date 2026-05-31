<script lang="ts">
  // Release calendar — ComicVine on-sale dates for a shipping week.
  // Self-owned row state, seeded from the load and re-fetched on range
  // change / refresh; the +page.ts load handles the initial paint.
  import { CalendarDays, ChevronLeft, ChevronRight, RefreshCw } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    bulkAddCalendarVolumesToPullList,
    getReleaseCalendar,
    type CalendarRow
  } from '$lib/api/releases';
  import { createSelection } from '$lib/createSelection.svelte';
  import { toast } from '$lib/stores/toast.svelte';
  import BulkActionBar from '$lib/components/BulkActionBar.svelte';
  import Button from '$lib/components/Button.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';

  let { data } = $props();

  let rows = $state<CalendarRow[]>([...data.rows]);
  let from = $state(data.from);
  let to = $state(data.to);
  let error = $state<ApiError | null>(null);
  let loading = $state(false);
  let refreshing = $state(false);
  let bulkBusy = $state(false);
  let pullFilter = $state<'all' | 'on' | 'off'>('all');

  // Bulk selection keyed by cv_volume_id — the pull-list unit. A volume
  // shipping several issues this week is one selection across its rows.
  const sel = createSelection<number>();

  const pullFilters: Array<{ value: 'all' | 'on' | 'off'; label: string }> = [
    { value: 'all', label: 'All' },
    { value: 'on', label: 'On pull list' },
    { value: 'off', label: 'Not on pull list' }
  ];

  const visibleRows = $derived(
    rows
      .filter((r) => {
        if (pullFilter === 'on') return r.on_pull_list;
        if (pullFilter === 'off') return !r.on_pull_list;
        return true;
      })
      .sort(
        (a, b) =>
          a.store_date.localeCompare(b.store_date) ||
          a.volume_name.localeCompare(b.volume_name)
      )
  );

  // The distinct volumes the bulk action can act on — visible rows not
  // already on the pull list. Drives select-all and the BulkActionBar.
  const selectableVolumeIds = $derived([
    ...new Set(visibleRows.filter((r) => !r.on_pull_list).map((r) => r.cv_volume_id))
  ]);

  function pad(n: number): string {
    return String(n).padStart(2, '0');
  }

  function addDays(isoDate: string, days: number): string {
    const [y, m, d] = isoDate.split('-').map(Number);
    const dt = new Date(y, m - 1, d + days);
    return `${dt.getFullYear()}-${pad(dt.getMonth() + 1)}-${pad(dt.getDate())}`;
  }

  function syncUrl(): void {
    const url = new URL(window.location.href);
    url.searchParams.set('from', from);
    url.searchParams.set('to', to);
    window.history.replaceState(null, '', url.pathname + url.search);
  }

  async function loadRange(refresh = false): Promise<void> {
    if (refresh) refreshing = true;
    else loading = true;
    error = null;
    try {
      rows = await getReleaseCalendar(from, to, refresh);
      syncUrl();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      loading = false;
      refreshing = false;
    }
  }

  function stepWeek(direction: -1 | 1): void {
    from = addDays(from, direction * 7);
    to = addDays(to, direction * 7);
    void loadRange();
  }

  async function bulkAdd(): Promise<void> {
    const ids = [...sel.selected];
    if (ids.length === 0) return;
    bulkBusy = true;
    error = null;
    try {
      const { results } = await bulkAddCalendarVolumesToPullList(ids);
      const added = results.filter((r) => r.status === 'added').length;
      const already = results.filter((r) => r.status === 'already_on_list').length;
      const failed = results.filter((r) => r.status === 'failed').length;
      // Flip every resolved volume's rows to on_pull_list (a volume can
      // span several rows); series_id rides along where the API gave one.
      const seriesByVolume = new Map(
        results
          .filter((r) => r.series_id != null)
          .map((r) => [r.cv_volume_id, r.series_id as number])
      );
      rows = rows.map((r) =>
        seriesByVolume.has(r.cv_volume_id)
          ? { ...r, series_id: seriesByVolume.get(r.cv_volume_id) ?? r.series_id, on_pull_list: true }
          : r
      );
      sel.clear();
      const parts = [`${added} added`];
      if (already > 0) parts.push(`${already} already on pull list`);
      if (failed > 0) parts.push(`${failed} failed`);
      const msg = `${parts.join(', ')}.`;
      if (failed > 0) toast.warning(msg);
      else toast.success(msg);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      bulkBusy = false;
    }
  }

  const busy = $derived(loading || refreshing);
  // Any in-flight mutation — gates the add controls so a bulk add and
  // a range reload can't race each other.
  const anyBusy = $derived(busy || bulkBusy);
</script>

<div class="mb-1 flex flex-wrap items-baseline justify-between gap-2">
  <h1 class="text-2xl font-bold">Release calendar</h1>
  <Button variant="secondary" onclick={() => loadRange(true)} loading={refreshing} disabled={busy}>
    <RefreshCw class="size-3.5" aria-hidden="true" />
    Refresh CV
  </Button>
</div>
<p class="mb-4 text-sm text-slate-600">
  Comics on sale this week, from ComicVine. Results are cached for an hour — "Refresh CV"
  forces a re-query.
</p>

{#if error}
  <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

<div class="mb-4 flex flex-wrap items-center gap-2">
  <Button variant="ghost" size="sm" onclick={() => stepWeek(-1)} disabled={busy}>
    <ChevronLeft class="size-4" aria-hidden="true" />Prev week
  </Button>
  <input
    type="date"
    bind:value={from}
    onchange={() => loadRange()}
    disabled={busy}
    aria-label="Range start"
    class="rounded-md border border-slate-300 px-2 py-1 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
  />
  <span class="text-slate-400">–</span>
  <input
    type="date"
    bind:value={to}
    onchange={() => loadRange()}
    disabled={busy}
    aria-label="Range end"
    class="rounded-md border border-slate-300 px-2 py-1 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
  />
  <Button variant="ghost" size="sm" onclick={() => stepWeek(1)} disabled={busy}>
    Next week<ChevronRight class="size-4" aria-hidden="true" />
  </Button>

  <div class="ml-auto flex flex-wrap gap-2" role="tablist" aria-label="Pull-list filter">
    {#each pullFilters as f (f.value)}
      <button
        type="button"
        role="tab"
        aria-selected={pullFilter === f.value}
        onclick={() => (pullFilter = f.value)}
        class="rounded-full border px-3 py-1 text-sm transition"
        class:border-slate-900={pullFilter === f.value}
        class:bg-slate-900={pullFilter === f.value}
        class:text-white={pullFilter === f.value}
        class:border-slate-300={pullFilter !== f.value}
        class:bg-white={pullFilter !== f.value}
        class:hover:bg-slate-50={pullFilter !== f.value}
      >
        {f.label}
      </button>
    {/each}
  </div>
</div>

{#if rows.length === 0}
  <EmptyState
    icon={CalendarDays}
    title="No releases in this range"
    message="ComicVine lists nothing on sale for these dates. Try another week."
  />
{:else if visibleRows.length === 0}
  <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
    No releases match this filter.
  </p>
{:else}
  {#if selectableVolumeIds.length > 0}
    <BulkActionBar
      count={sel.size}
      allSelected={sel.allSelected(selectableVolumeIds)}
      someSelected={sel.someSelected(selectableVolumeIds)}
      onToggleAll={() => sel.toggleAll(selectableVolumeIds)}
      selectAllLabel="Select all addable volumes"
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
  <div class="overflow-hidden rounded-lg border border-slate-200 bg-white">
    <table class="w-full text-sm">
      <thead class="border-b border-slate-200 bg-slate-50 text-left text-xs text-slate-500">
        <tr>
          <th class="w-8 px-4 py-2"></th>
          <th class="px-4 py-2 font-medium">On sale</th>
          <th class="px-4 py-2 font-medium">Cover</th>
          <th class="px-4 py-2 font-medium">Series</th>
          <th class="px-4 py-2 font-medium">Issue</th>
          <th class="px-4 py-2"></th>
        </tr>
      </thead>
      <tbody class="divide-y divide-slate-100">
        {#each visibleRows as row (row.cv_issue_id)}
          <tr>
            <td class="px-4 py-2">
              {#if !row.on_pull_list}
                <input
                  type="checkbox"
                  class="rounded border-slate-300"
                  checked={sel.has(row.cv_volume_id)}
                  onchange={() => sel.toggle(row.cv_volume_id)}
                  disabled={anyBusy}
                  aria-label={`Select ${row.volume_name || 'Unknown series'}`}
                />
              {/if}
            </td>
            <td class="whitespace-nowrap px-4 py-2 font-mono text-xs text-slate-600">
              {row.store_date}
            </td>
            <td class="px-4 py-2">
              {#if row.cover_url}
                <img
                  src={row.cover_url}
                  alt=""
                  class="size-10 rounded bg-slate-100 object-cover"
                  loading="lazy"
                />
              {:else}
                <div class="size-10 rounded bg-slate-100" aria-hidden="true"></div>
              {/if}
            </td>
            <td class="px-4 py-2">
              <a
                href={row.site_detail_url}
                target="_blank"
                rel="noreferrer"
                class="font-medium text-blue-600 hover:underline"
              >
                {row.volume_name || 'Unknown series'}
              </a>
            </td>
            <td class="px-4 py-2 font-mono text-slate-600">#{row.issue_number}</td>
            <td class="px-4 py-2">
              <div class="flex justify-end">
                {#if row.on_pull_list}
                  <span
                    class="rounded bg-emerald-50 px-1.5 py-0.5 text-xs font-medium text-emerald-700"
                  >
                    On pull list
                  </span>
                {/if}
              </div>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
