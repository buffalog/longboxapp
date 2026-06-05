<script lang="ts">
  import { onMount } from 'svelte';
  import { BookOpen, FileImage } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { getActivity, type DashboardActivity } from '$lib/api/dashboard';
  import { getReconcileCounts, type ReconcileCounts } from '$lib/api/reconcile';
  import {
    getReleasesOfNote,
    getThisWeeksPulls,
    type PullThisWeek,
    type ReleaseOfNote
  } from '$lib/api/releases';
  import { getStats } from '$lib/api/stats';
  import { triggerFullScan } from '$lib/api/scans';
  import { scanStatus } from '$lib/stores/scanStatus.svelte';
  import { formatRelative } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import LoadingSpinner from '$lib/components/LoadingSpinner.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import ReconciliationBanner from '$lib/components/ReconciliationBanner.svelte';
  import ReleasesOfNoteWidget from '$lib/components/ReleasesOfNoteWidget.svelte';
  import ThisWeeksPullsWidget from '$lib/components/ThisWeeksPullsWidget.svelte';
  import type { Stats } from '$lib/types';

  let { data } = $props();

  let stats = $state<Stats | null>(null);
  let activity = $state<DashboardActivity | null>(null);
  let loading = $state(true);
  let error = $state<ApiError | null>(null);
  let triggering = $state(false);
  // Library Tidy counts for the nudge banner. Fetched independently of
  // the critical dashboard data — see onMount.
  let reconcileCounts = $state<ReconcileCounts | null>(null);
  // This week's releases-of-note — also fetched independently.
  let releasesOfNote = $state<ReleaseOfNote[]>([]);
  // This week's pulls for subscribed series — likewise independent.
  let thisWeeksPulls = $state<PullThisWeek[]>([]);

  // libraryRoot comes from the layout's load(). It's null only if
  // /api/library-roots failed at boot — which would have also surfaced
  // via the global ErrorBanner.
  const libraryRootId = $derived(data.libraryRoot?.id ?? null);

  // Needs-attention tile aggregates the two failure surfaces the
  // /needs-attention page shows: post-processor stuck files +
  // failure-class pull attempts. Both counts arrive in the single
  // /api/stats payload now — previously this required three separate
  // HTTP round-trips (/postprocess/pending, /needs-attention/pull-
  // failures, /pull-list), which is the dashboard-slowness that
  // FIX 3 from the kickoff brief was about.
  const needsAttentionCount = $derived(
    (stats?.pending_interventions_count ?? 0) + (stats?.pull_failures_count ?? 0)
  );

  onMount(async () => {
    // The reconciliation banner is a non-critical nudge — fetch its
    // counts independently so a failure here never trips the page error
    // or hides the stats grid. On failure: no banner, silently.
    getReconcileCounts()
      .then((c) => {
        reconcileCounts = c;
      })
      .catch(() => {});

    // The releases-of-note and this-week's-pulls widgets are likewise
    // non-critical — a failure means no widget, never a page error.
    getReleasesOfNote()
      .then((r) => {
        releasesOfNote = r;
      })
      .catch(() => {});
    getThisWeeksPulls()
      .then((r) => {
        thisWeeksPulls = r;
      })
      .catch(() => {});

    try {
      // Two calls: stats (everything for the tiles) + activity (the
      // recent-completed feed). Pull-list / pull-failures / pending-
      // interventions counts ride along inside the stats payload — the
      // three /api/* endpoints they used to come from are still
      // available, just not consumed here.
      const [s, a] = await Promise.all([getStats(), getActivity(6)]);
      stats = s;
      activity = a;
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      loading = false;
    }
  });

  async function triggerScan(): Promise<void> {
    if (libraryRootId === null) return;
    triggering = true;
    error = null;
    try {
      await triggerFullScan(libraryRootId);
      await scanStatus.refresh();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      triggering = false;
    }
  }

  // "Last scan" on the dashboard means: most recent successfully-completed
  // scan that did real work. A no-op rescan-unmatched against a zero-
  // needs-review catalog still gets a `completed` row with files_seen=0;
  // surfacing it as "Last scan completed Xs ago" misrepresents activity.
  // Full scans always count even if they processed nothing, since the
  // disk walk itself is the work.
  const lastScanTime = $derived(
    scanStatus.recent.find(
      (r) => r.status === 'completed' && (r.files_seen > 0 || r.kind === 'full')
    )?.finished_at ?? null
  );
</script>

<h1 class="mb-4 text-2xl font-bold">Dashboard</h1>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

{#if reconcileCounts}
  <ReconciliationBanner
    transitionCount={reconcileCounts.phantoms_with_transition}
    untrackedCount={reconcileCounts.untracked_folders}
  />
{/if}

{#if loading}
  <LoadingSpinner />
{:else if stats}
  <section class="mb-6 grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-8">
    <a
      href="/series"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
    >
      <div class="text-xs uppercase text-slate-500">Series</div>
      <div class="text-2xl font-semibold">{stats.total_series}</div>
    </a>
    <a
      href="/series"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
    >
      <div class="text-xs uppercase text-slate-500">Issues</div>
      <div class="text-2xl font-semibold">{stats.total_issues}</div>
    </a>
    <a
      href="/files?status=owned"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
    >
      <div class="text-xs uppercase text-status-owned">Owned</div>
      <div class="text-2xl font-semibold">{stats.owned_files}</div>
    </a>
    <a
      href="/files?status=needs_review"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
    >
      <div class="text-xs uppercase text-status-needs_review">Needs review</div>
      <div class="text-2xl font-semibold">{stats.needs_review_files}</div>
    </a>
    <a
      href="/files?status=unmatched"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
    >
      <div class="text-xs uppercase text-status-unmatched">Unmatched</div>
      <div class="text-2xl font-semibold">{stats.unmatched_files}</div>
    </a>
    <a
      href="/missing"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
    >
      <div class="text-xs uppercase text-status-missing">Missing</div>
      <div class="text-2xl font-semibold">{stats.missing_issues}</div>
    </a>
    <a
      href="/needs-attention"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
      title="Failed pulls and files that could not be processed automatically"
    >
      <div class="text-xs uppercase text-status-needs_review">Needs attention</div>
      <div class="text-2xl font-semibold">{needsAttentionCount}</div>
    </a>
    <a
      href="/releases/pull-list"
      class="rounded-lg border border-slate-200 bg-white p-3 transition hover:bg-slate-50"
      title="Series subscribed for auto-download"
    >
      <div class="text-xs uppercase text-slate-500">Pull list</div>
      <div class="text-2xl font-semibold">{stats.pull_list_count}</div>
    </a>
  </section>

  <section class="mb-6 rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">Scan status</h2>
    {#if scanStatus.current}
      <p class="text-sm">
        <strong>Scanning…</strong> Started {formatRelative(scanStatus.current.started_at)} ·
        <a class="text-blue-600 hover:underline" href="/scans">details</a>
      </p>
    {:else if lastScanTime}
      <p class="text-sm">
        Last scan completed {formatRelative(lastScanTime)} ·
        <a class="text-blue-600 hover:underline" href="/scans">history</a>
      </p>
    {:else}
      <p class="text-sm text-slate-600">No scans yet. Run one to populate the catalog.</p>
    {/if}
  </section>

  <section class="mb-6 flex flex-wrap gap-3">
    <Button onclick={() => (window.location.href = '/add')}>Add series</Button>
    <Button
      variant="secondary"
      onclick={triggerScan}
      loading={triggering}
      disabled={!!scanStatus.current || libraryRootId === null}
    >
      {scanStatus.current ? 'Scanning…' : 'Scan library'}
    </Button>
  </section>

  {#if stats.missing_issues > 0}
    <section class="mb-6">
      <a
        href="/missing"
        class="block rounded-lg border border-slate-200 bg-white p-3 hover:bg-slate-50"
      >
        <div class="flex items-baseline justify-between gap-3">
          <div class="text-sm">
            <span class="font-semibold text-status-missing">{stats.missing_issues}</span>
            issue{stats.missing_issues === 1 ? '' : 's'} missing across
            <span class="font-semibold">{stats.series_with_missing}</span>
            series
          </div>
          <span class="text-xs font-medium text-blue-600">view all →</span>
        </div>
      </a>
    </section>
  {/if}

  <!-- Releases widgets: each self-hides when it has nothing to show. -->
  <ThisWeeksPullsWidget rows={thisWeeksPulls} />
  <ReleasesOfNoteWidget rows={releasesOfNote} />

  {#if activity}
    <section class="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <div class="rounded-lg border border-slate-200 bg-white p-4">
        <h2 class="mb-3 text-base font-semibold">Recently added series</h2>
        {#if activity.recent_series.length === 0}
          <p class="text-sm text-slate-500">
            No series in the watchlist yet.
            <a class="text-blue-600 hover:underline" href="/add">Add one</a>.
          </p>
        {:else}
          <ul class="space-y-2">
            {#each activity.recent_series as s (s.id)}
              <li>
                <a
                  href={`/series/${s.id}`}
                  class="flex items-center gap-3 rounded-md p-1 hover:bg-slate-50"
                >
                  <div class="size-12 flex-shrink-0 overflow-hidden rounded bg-slate-100">
                    {#if s.cover_url}
                      <img src={s.cover_url} alt="" class="size-full object-cover" loading="lazy" />
                    {:else}
                      <div class="flex size-full items-center justify-center text-slate-400">
                        <BookOpen class="size-5" aria-hidden="true" />
                      </div>
                    {/if}
                  </div>
                  <div class="min-w-0 flex-1">
                    <div class="truncate text-sm font-medium">{s.title}</div>
                    <div class="truncate text-xs text-slate-500">
                      {s.start_year ?? '—'}{s.publisher ? ` · ${s.publisher}` : ''}
                    </div>
                  </div>
                  <span class="rounded bg-slate-100 px-1.5 py-0.5 text-xs font-medium">
                    {s.owned_count}/{s.total_count}
                  </span>
                </a>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      <div class="rounded-lg border border-slate-200 bg-white p-4">
        <h2 class="mb-3 text-base font-semibold">Recently completed issues</h2>
        {#if activity.recent_matches.length === 0}
          <p class="text-sm text-slate-500">
            No recent matches yet. Match files via
            <a class="text-blue-600 hover:underline" href="/files">/files</a>
            to populate this.
          </p>
        {:else}
          <ul class="space-y-2">
            {#each activity.recent_matches as m (m.file_id)}
              <li>
                <a
                  href={`/series/${m.series.id}`}
                  class="flex items-center gap-3 rounded-md p-1 hover:bg-slate-50"
                >
                  <div class="size-12 flex-shrink-0 overflow-hidden rounded bg-slate-100">
                    {#if m.issue.cover_url}
                      <img src={m.issue.cover_url} alt="" class="size-full object-cover" loading="lazy" />
                    {:else}
                      <div class="flex size-full items-center justify-center text-slate-400">
                        <FileImage class="size-5" aria-hidden="true" />
                      </div>
                    {/if}
                  </div>
                  <div class="min-w-0 flex-1">
                    <div class="truncate text-sm font-medium">
                      {m.series.title} <span class="font-mono text-slate-500">#{m.issue.number}</span>
                    </div>
                    <div class="truncate font-mono text-xs text-slate-500" title={m.path_relative}>
                      {m.path_relative}
                    </div>
                  </div>
                  <span class="whitespace-nowrap text-xs text-slate-500">
                    {formatRelative(m.matched_at)}
                  </span>
                </a>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </section>
  {/if}
{/if}
