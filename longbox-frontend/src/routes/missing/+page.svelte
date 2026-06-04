<script lang="ts">
  import { goto } from '$app/navigation';
  import { CircleSlash, FileImage, Search } from 'lucide-svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import AlphaScrubber from '$lib/components/AlphaScrubber.svelte';
  import Button from '$lib/components/Button.svelte';
  import { ApiError } from '$lib/api/client';
  import { searchAllMissing, type MissingIssue, type MissingSort } from '$lib/api/missing';
  import { searchSeriesNow } from '$lib/api/pull';
  import { isSolicited } from '$lib/solicitation';
  import { toast } from '$lib/stores/toast.svelte';

  let { data } = $props();
  let groupsEl: HTMLElement | null = $state(null);
  let searchingAll = $state(false);
  // Set of series ids currently mid-search via the per-row button. A
  // Map clone keyed by series id would also work; a plain Set is enough
  // since the button cares about presence, not metadata.
  let searchingSeries = $state<Set<number>>(new Set());

  function setSort(s: MissingSort): void {
    const url = new URL(window.location.href);
    if (s === 'series') url.searchParams.delete('sort');
    else url.searchParams.set('sort', s);
    void goto(url.pathname + url.search, { replaceState: true });
  }

  function setSeriesFilter(value: string): void {
    const url = new URL(window.location.href);
    if (value === '') url.searchParams.delete('series_id');
    else url.searchParams.set('series_id', value);
    void goto(url.pathname + url.search, { replaceState: true });
  }

  // "Missing for X" — best-effort age from cover_date. Solicited issues
  // (cover_date today-or-future, not shipped yet) show "Solicited"
  // rather than a nonsense negative age — via the shared `isSolicited`
  // predicate, the same one /series/[id] uses. Null cover_date → "—".
  function missingAge(coverDate: string | null): string {
    if (!coverDate) return '—';
    if (isSolicited(coverDate)) return 'Solicited';
    const cover = Date.parse(coverDate + 'T00:00:00Z');
    if (Number.isNaN(cover)) return '—';
    const diffMs = Date.now() - cover;
    const days = Math.floor(diffMs / (1000 * 60 * 60 * 24));
    if (days < 31) return `${days}d`;
    const months = Math.floor(days / 30);
    if (months < 24) return `${months}mo`;
    const years = (months / 12).toFixed(1).replace(/\.0$/, '');
    return `${years}y`;
  }

  // Grouped render only makes sense for sort=series. When sort=cover_date
  // we render a single flat table.
  interface MissingGroup {
    series_id: number;
    series_title: string;
    series_sort_title: string;
    start_year: number | null;
    rows: MissingIssue[];
  }

  // Partition first: solicited issues are not actionable (the pull
  // engine has nothing to search for — they haven't shipped yet). Each
  // surface — header count, global Search-all button, per-series rows,
  // group counts — consults the actionable subset. The solicited
  // subset renders as its own section at the bottom for visibility
  // without ever counting against "searchable."
  const actionableRows = $derived(
    data.missing.missing.filter((m) => !isSolicited(m.cover_date))
  );
  const solicitedRows = $derived(
    data.missing.missing.filter((m) => isSolicited(m.cover_date))
  );

  // Map-based grouper keyed by series.id — robust to any sort order in
  // the API response. The previous consecutive-merge loop produced
  // duplicate series_id groups when the API interleaved rows from two
  // distinct series that shared a title (e.g. multiple "The Department
  // of Truth" volumes), which crashed `{#each ... (g.series_id)}` with
  // a Svelte each_key_duplicate error.
  function groupBySeries(rows: MissingIssue[]): MissingGroup[] {
    const map = new Map<number, MissingGroup>();
    for (const m of rows) {
      let g = map.get(m.series.id);
      if (!g) {
        g = {
          series_id: m.series.id,
          series_title: m.series.title,
          series_sort_title: m.series.sort_title,
          start_year: m.series.start_year,
          rows: []
        };
        map.set(m.series.id, g);
      }
      g.rows.push(m);
    }
    return Array.from(map.values());
  }

  const groups = $derived.by<MissingGroup[]>(() =>
    data.sort === 'series' ? groupBySeries(actionableRows) : []
  );
  const solicitedGroups = $derived.by<MissingGroup[]>(() =>
    data.sort === 'series' ? groupBySeries(solicitedRows) : []
  );

  async function handleSearchAll(): Promise<void> {
    if (searchingAll || actionableRows.length === 0) return;
    searchingAll = true;
    try {
      const summary = await searchAllMissing();
      const skippedPhrase =
        summary.skipped_solicited > 0
          ? ` (${summary.skipped_solicited} solicited skipped)`
          : '';
      toast.success(
        `Search dispatched for ${summary.searched} missing issue${
          summary.searched === 1 ? '' : 's'
        }${skippedPhrase}.`
      );
    } catch (e) {
      const message =
        e instanceof ApiError ? e.message : 'Could not start the search.';
      toast.warning(message);
    } finally {
      searchingAll = false;
    }
  }

  async function handleSearchSeries(seriesId: number, seriesTitle: string): Promise<void> {
    if (searchingSeries.has(seriesId)) return;
    // Svelte 5 runes treat the Set as reactive only on reassignment.
    searchingSeries = new Set(searchingSeries).add(seriesId);
    try {
      await searchSeriesNow(seriesId);
      toast.success(`Search started for ${seriesTitle}.`);
    } catch (e) {
      const message =
        e instanceof ApiError ? e.message : 'Could not start the search.';
      toast.warning(message);
    } finally {
      const next = new Set(searchingSeries);
      next.delete(seriesId);
      searchingSeries = next;
    }
  }
</script>

<header class="mb-4 flex flex-wrap items-baseline justify-between gap-3">
  <h1 class="text-2xl font-bold">Missing</h1>
  <div class="flex items-center gap-3">
    <span class="text-sm text-slate-500">
      {data.missing.total} missing issue{data.missing.total === 1 ? '' : 's'}
      {#if solicitedRows.length > 0}
        <span class="text-slate-400">({solicitedRows.length} solicited)</span>
      {/if}
    </span>
    {#if actionableRows.length > 0}
      <Button
        variant="primary"
        size="sm"
        loading={searchingAll}
        disabled={searchingAll}
        onclick={handleSearchAll}
      >
        <Search class="size-3.5" aria-hidden="true" />
        Search {actionableRows.length} missing
      </Button>
    {/if}
  </div>
</header>

{#if data.missing.total === 0 && data.seriesIdFilter === null}
  <EmptyState
    icon={CircleSlash}
    title="Nothing missing"
    message="Every issue your catalog knows about is matched to a file. Add more series via /add to see gaps to fill."
  />
{:else}
  <div class="mb-4 flex flex-wrap items-center gap-3 text-sm">
    <label class="inline-flex items-center gap-2">
      <span class="text-slate-600">Series</span>
      <select
        class="rounded-md border border-slate-300 bg-white px-2 py-1 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        value={data.seriesIdFilter !== null ? String(data.seriesIdFilter) : ''}
        onchange={(e) => setSeriesFilter((e.target as HTMLSelectElement).value)}
      >
        <option value="">All series</option>
        {#each data.allSeries as s (s.id)}
          <option value={String(s.id)}>
            {s.title}{s.start_year ? ` (${s.start_year})` : ''}
          </option>
        {/each}
      </select>
    </label>

    <label class="inline-flex items-center gap-2">
      <span class="text-slate-600">Sort</span>
      <select
        class="rounded-md border border-slate-300 bg-white px-2 py-1 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        value={data.sort}
        onchange={(e) => setSort((e.target as HTMLSelectElement).value as MissingSort)}
      >
        <option value="series">Series</option>
        <option value="cover_date">Cover date</option>
      </select>
    </label>
  </div>

  {#if data.missing.total === 0}
    <p class="text-sm text-slate-500">No missing issues in this view.</p>
  {:else if data.sort === 'cover_date'}
    {#if actionableRows.length > 0}
      <ul class="space-y-2">
        {#each actionableRows as m (m.issue_id)}
          <li>
            <a
              href={`/series/${m.series.id}`}
              class="flex items-center gap-3 rounded-md border border-slate-200 bg-white p-2 hover:bg-slate-50"
            >
              <div class="size-12 flex-shrink-0 overflow-hidden rounded bg-slate-100">
                {#if m.cover_url}
                  <img src={m.cover_url} alt="" class="size-full object-cover" loading="lazy" />
                {:else}
                  <div class="flex size-full items-center justify-center text-slate-400">
                    <FileImage class="size-5" aria-hidden="true" />
                  </div>
                {/if}
              </div>
              <div class="min-w-0 flex-1">
                <div class="truncate text-sm font-medium">
                  {m.series.title}
                  <span class="font-mono text-slate-500">#{m.number}</span>
                </div>
                <div class="truncate text-xs text-slate-500">
                  {m.title ?? '—'}{m.cover_date ? ` · ${m.cover_date}` : ''}
                </div>
              </div>
              <span class="whitespace-nowrap text-xs text-slate-500">{missingAge(m.cover_date)}</span>
            </a>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="text-sm text-slate-500">
        Every missing issue in this view is solicited — see below.
      </p>
    {/if}
  {:else}
    {#if groups.length > 0}
      <div bind:this={groupsEl} class="space-y-4">
        {#each groups as g (g.series_id)}
          <section
            id={`missing-series-${g.series_id}`}
            style="scroll-margin-top: var(--sticky-nav-height, 0);"
            class="rounded-lg border border-slate-200 bg-white p-3"
          >
            <h2 class="mb-2 flex flex-wrap items-center justify-between gap-2 text-sm font-semibold">
              <span>
                <a class="hover:underline" href={`/series/${g.series_id}`}>
                  {g.series_title}{g.start_year ? ` (${g.start_year})` : ''}
                </a>
                <span class="font-normal text-slate-500">· {g.rows.length} missing</span>
              </span>
              <Button
                variant="secondary"
                size="sm"
                loading={searchingSeries.has(g.series_id)}
                disabled={searchingSeries.has(g.series_id) || searchingAll}
                onclick={() => handleSearchSeries(g.series_id, g.series_title)}
              >
                <Search class="size-3" aria-hidden="true" />
                Search
              </Button>
            </h2>
            <ul class="divide-y divide-slate-100">
              {#each g.rows as m (m.issue_id)}
                <li class="flex items-center gap-3 py-1.5">
                  <div class="size-10 flex-shrink-0 overflow-hidden rounded bg-slate-100">
                    {#if m.cover_url}
                      <img src={m.cover_url} alt="" class="size-full object-cover" loading="lazy" />
                    {:else}
                      <div class="flex size-full items-center justify-center text-slate-400">
                        <FileImage class="size-4" aria-hidden="true" />
                      </div>
                    {/if}
                  </div>
                  <span class="w-16 font-mono text-sm">#{m.number}</span>
                  <span class="min-w-0 flex-1 truncate text-sm">{m.title ?? '—'}</span>
                  <span class="w-24 whitespace-nowrap text-xs text-slate-500">
                    {m.cover_date ?? '—'}
                  </span>
                  <span class="w-16 whitespace-nowrap text-right text-xs text-slate-500">
                    {missingAge(m.cover_date)}
                  </span>
                </li>
              {/each}
            </ul>
          </section>
        {/each}
      </div>
      <!-- Scrubber only renders for sort=series (group headers double as
           anchors). Cover-date sort doesn't expose alphabetical structure
           per brief; the {#if} above hides this entire branch then. -->
      <AlphaScrubber
        items={groups}
        getSortKey={(g: MissingGroup) => g.series_sort_title}
        getElementId={(g: MissingGroup) => `missing-series-${g.series_id}`}
        listEl={groupsEl}
      />
    {:else}
      <p class="text-sm text-slate-500">
        Every missing issue in this view is solicited — see below.
      </p>
    {/if}
  {/if}

  <!-- ============= Solicited ============= -->
  {#if solicitedRows.length > 0}
    <section class="mt-8">
      <header class="mb-2 flex items-baseline justify-between gap-2">
        <h2 class="text-lg font-semibold text-slate-700">Solicited</h2>
        <span class="text-xs text-slate-500">
          {solicitedRows.length} not yet shipped — no indexer search until cover date passes
        </span>
      </header>
      {#if data.sort === 'cover_date'}
        <ul class="space-y-2">
          {#each solicitedRows as m (m.issue_id)}
            <li>
              <a
                href={`/series/${m.series.id}`}
                class="flex items-center gap-3 rounded-md border border-slate-200 bg-slate-50 p-2 opacity-75 hover:opacity-100"
              >
                <div class="size-12 flex-shrink-0 overflow-hidden rounded bg-slate-100">
                  {#if m.cover_url}
                    <img src={m.cover_url} alt="" class="size-full object-cover" loading="lazy" />
                  {:else}
                    <div class="flex size-full items-center justify-center text-slate-400">
                      <FileImage class="size-5" aria-hidden="true" />
                    </div>
                  {/if}
                </div>
                <div class="min-w-0 flex-1">
                  <div class="truncate text-sm font-medium">
                    {m.series.title}
                    <span class="font-mono text-slate-500">#{m.number}</span>
                  </div>
                  <div class="truncate text-xs text-slate-500">
                    {m.title ?? '—'}{m.cover_date ? ` · ${m.cover_date}` : ''}
                  </div>
                </div>
                <span class="whitespace-nowrap text-xs text-slate-500">Solicited</span>
              </a>
            </li>
          {/each}
        </ul>
      {:else}
        <div class="space-y-4">
          {#each solicitedGroups as g (g.series_id)}
            <section class="rounded-lg border border-slate-200 bg-slate-50 p-3 opacity-90">
              <h3 class="mb-2 text-sm font-semibold">
                <a class="hover:underline" href={`/series/${g.series_id}`}>
                  {g.series_title}{g.start_year ? ` (${g.start_year})` : ''}
                </a>
                <span class="font-normal text-slate-500">· {g.rows.length} solicited</span>
              </h3>
              <ul class="divide-y divide-slate-100">
                {#each g.rows as m (m.issue_id)}
                  <li class="flex items-center gap-3 py-1.5">
                    <div class="size-10 flex-shrink-0 overflow-hidden rounded bg-slate-100">
                      {#if m.cover_url}
                        <img src={m.cover_url} alt="" class="size-full object-cover" loading="lazy" />
                      {:else}
                        <div class="flex size-full items-center justify-center text-slate-400">
                          <FileImage class="size-4" aria-hidden="true" />
                        </div>
                      {/if}
                    </div>
                    <span class="w-16 font-mono text-sm">#{m.number}</span>
                    <span class="min-w-0 flex-1 truncate text-sm">{m.title ?? '—'}</span>
                    <span class="w-24 whitespace-nowrap text-xs text-slate-500">
                      {m.cover_date ?? '—'}
                    </span>
                    <span class="w-16 whitespace-nowrap text-right text-xs text-slate-500">
                      Solicited
                    </span>
                  </li>
                {/each}
              </ul>
            </section>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
{/if}
