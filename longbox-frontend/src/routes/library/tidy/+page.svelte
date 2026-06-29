<script lang="ts">
  // Library Tidy — reconcile the catalog against disk in both
  // directions. Self-owned list state, seeded from the load data and
  // maintained from each mutation's response (the pull-list pattern).
  //
  // A.9 Step 6a — the two bulk sections run on the shared BulkActionBar
  // + createSelection primitives (Step 4), and untracked folders gain a
  // shallow bulk-convert (folder → tracked series, no ComicVine).
  import { Folder, Sparkles } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    setSeriesCvId,
    type EnrichmentQueueRow,
    type EnrichmentReviewOutcome
  } from '$lib/api/enrichment';
  import {
    addFolders,
    convertFolders,
    dismissFolders,
    keepPhantom,
    unkeepPhantom,
    mergeDuplicates,
    type DiscoveredFolder,
    type DuplicatePair,
    type PhantomSeries
  } from '$lib/api/reconcile';
  import { deleteSeries } from '$lib/api/series';
  import { createSelection } from '$lib/createSelection.svelte';
  import { formatDateTime } from '$lib/format';
  import { searchHintFromPath } from '$lib/match_hint';
  import { toast } from '$lib/stores/toast.svelte';
  import BulkActionBar from '$lib/components/BulkActionBar.svelte';
  import Button from '$lib/components/Button.svelte';
  import CvSearchInput from '$lib/components/CvSearchInput.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import type { SeriesSearchResult } from '$lib/types';

  let { data } = $props();

  // Phantom side: hold `all_zero_owned` as the single source of truth.
  // The transition subsection is a $derived view of this list, NOT a
  // separately tracked list — see the $derived block below.
  let phantoms = $state<PhantomSeries[]>([...data.phantoms.all_zero_owned]);
  // The "Kept series" set — phantoms exempted from tidy. Held as its own
  // list (the API filters them out of `phantoms`); Keep/Un-keep splice a
  // row between the two lists locally, no refetch.
  let kept = $state<PhantomSeries[]>([...data.kept]);
  let untracked = $state<DiscoveredFolder[]>([...data.untracked]);
  // Mutable copy of the enrichment review queue so a successful pick
  // can be removed locally without a refetch — same pattern as
  // phantoms / untracked above.
  let enrichmentQueue = $state<EnrichmentQueueRow[]>([...data.enrichmentQueue]);
  /** Mutable copy of the duplicate-pair list so a successful merge
   *  can drop the row locally without a re-fetch. */
  let duplicates = $state<DuplicatePair[]>([...data.duplicates]);
  /** Per-pair in-flight set, keyed by `${target_id}:${source_id}` so a
   *  parallel merge of an unrelated pair isn't blocked. */
  let mergeInFlight = $state<Set<string>>(new Set());

  // Destructive-confirmation state. Every "Remove now" click sets a
  // target (or opens the bulk modal); the modal is the only path to
  // actually firing the delete. Per the spec, the confirm button
  // routes through DELETE /api/series/:id?delete_files=true — the
  // backend's per-call safety net handles a missing folder gracefully,
  // so phantom rows that never had a folder on disk are fine.
  let phantomDeleteTarget = $state<PhantomSeries | null>(null);
  let phantomDeleting = $state(false);
  let bulkPhantomDeleteOpen = $state(false);
  let bulkPhantomDeleting = $state(false);
  // Per-row in-flight tracking so the row's CvSearchInput disables
  // (and the row's UI shows progress) without locking out the whole
  // queue while one PATCH is in flight. Keyed by series id.
  let enrichmentPickInFlight = $state<Set<number>>(new Set());

  /** Per-row cv_id_in_use collision state. The PATCH fails with this
   *  code when the user-picked CV id is already claimed by ANOTHER
   *  series row — the common cause is a duplicate created by the
   *  untracked-folder import for a series that was already in the
   *  catalog under a slightly different folder name. The fix is to
   *  delete THIS (duplicate) queue row, not the existing one. */
  interface CollisionState {
    existingSeriesId: number;
    existingSeriesTitle: string;
  }
  let enrichmentCollisions = $state<Map<number, CollisionState>>(new Map());
  let enrichmentDeleteInFlight = $state<Set<number>>(new Set());

  let error = $state<ApiError | null>(null);
  let busy = $state(false);

  // Bulk selection — one per bulk section, on the shared primitive.
  const phantomSel = createSelection<number>();
  const folderSel = createSelection<string>();

  // Add-to-LongBox modal.
  let addModalFolder = $state<DiscoveredFolder | null>(null);
  let addCvSelected = $state<SeriesSearchResult | null>(null);
  let addSubmitting = $state(false);
  let addError = $state<string | null>(null);

  // The zero-owned phantom set is partitioned into four DISJOINT
  // subsections — every phantom renders in exactly one. Precedence
  // (highest first): a row scheduled for automatic removal outranks a
  // transition row (lost files), which outranks an awaiting-first-
  // download row (on the pull list, never downloaded), which outranks a
  // plain empty series. Deriving all four from the single `phantoms`
  // list — rather than the API's overlapping `with_transition` /
  // `all_zero_owned` lists — keeps "Keep" a pure local mutation: clear
  // a row's signals and it reactively slides between subsections with
  // no refetch. Do not "fix" this into rendering the API's overlapping
  // lists directly — the disjoint partition is intentional.
  const scheduledForRemoval = $derived(phantoms.filter((p) => p.auto_tidy_due_at !== null));
  const transitionPhantoms = $derived(
    phantoms.filter((p) => p.auto_tidy_due_at === null && p.last_matched_count > 0)
  );
  const awaitingFirstDownload = $derived(
    phantoms.filter(
      (p) =>
        p.auto_tidy_due_at === null && p.last_matched_count === 0 && p.awaiting_first_download
    )
  );
  const steadyStatePhantoms = $derived(
    phantoms.filter(
      (p) =>
        p.auto_tidy_due_at === null && p.last_matched_count === 0 && !p.awaiting_first_download
    )
  );

  // Id lists the BulkActionBar's select-all toggles against.
  const steadyIds = $derived(steadyStatePhantoms.map((p) => p.id));
  const untrackedNames = $derived(untracked.map((f) => f.folder_name));

  const addHint = $derived(
    addModalFolder ? searchHintFromPath(`${addModalFolder.folder_name}/_.cbz`) : ''
  );

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

  // --- phantom mutations ---------------------------------------------
  function handleKeep(seriesId: number): Promise<void> {
    return run(async () => {
      await keepPhantom(seriesId);
      // Keep is now durable: the series leaves the phantom list entirely
      // and moves into "Kept series". Mirror that locally — splice the
      // row out of `phantoms` and into `kept`, with its signals cleared
      // to match what the endpoint wrote.
      const row = phantoms.find((p) => p.id === seriesId);
      phantoms = phantoms.filter((p) => p.id !== seriesId);
      phantomSel.discard(seriesId);
      if (row) {
        kept = [...kept, { ...row, last_matched_count: 0, auto_tidy_due_at: null }].sort((a, b) =>
          a.sort_title.localeCompare(b.sort_title)
        );
      }
      toast.success('Kept this series. It won’t be tidied.');
    });
  }

  function handleUnkeep(seriesId: number): Promise<void> {
    return run(async () => {
      await unkeepPhantom(seriesId);
      // Reverse of Keep: the series returns to the phantom list. It lands
      // in the steady-state bucket (signals already at 0/null) and the
      // scanner re-evaluates it from scratch on the next scan.
      const row = kept.find((p) => p.id === seriesId);
      kept = kept.filter((p) => p.id !== seriesId);
      if (row) {
        phantoms = [...phantoms, row].sort((a, b) => a.sort_title.localeCompare(b.sort_title));
      }
      toast.success('No longer keeping this series.');
    });
  }

  /// Open the per-row confirmation modal. The actual delete happens
  /// only after the user confirms (see `confirmRemovePhantom`).
  function requestRemovePhantom(p: PhantomSeries): void {
    phantomDeleteTarget = p;
  }

  /// Folder name the backend will compute and try to remove.
  /// Mirrors `series_folder_path` in routes/series.rs exactly.
  function folderNameFor(p: { title: string; start_year: number | null }): string {
    return p.start_year ? `${p.title} (${p.start_year})` : p.title;
  }

  async function confirmRemovePhantom(): Promise<void> {
    if (!phantomDeleteTarget) return;
    const target = phantomDeleteTarget;
    phantomDeleting = true;
    error = null;
    try {
      // delete_files=true so an orphan folder (a phantom's bytes-on-
      // disk are by definition not catalog-linked, but the folder may
      // still exist) is cleaned up in the same call. Backend's
      // folder-absent branch makes this safe when no folder exists.
      await deleteSeries(target.id, { deleteFiles: true });
      phantoms = phantoms.filter((p) => p.id !== target.id);
      phantomSel.discard(target.id);
      toast.success('Series removed from the catalog.');
      phantomDeleteTarget = null;
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      phantomDeleting = false;
    }
  }

  /// Open the bulk-confirmation modal. The shared `phantomSel`
  /// selection is the source of truth; the modal renders count +
  /// list-preview from it.
  function requestBulkRemovePhantoms(): void {
    if (phantomSel.selected.size === 0) return;
    bulkPhantomDeleteOpen = true;
  }

  /// Each selected phantom is deleted via the per-series endpoint with
  /// `delete_files=true` — the spec's single canonical delete path —
  /// rather than the historical `/reconcile/phantoms/bulk` endpoint
  /// (DB-only, doesn't touch disk). Fanning out via Promise.all keeps
  /// the wall-clock close to one round-trip; the backend's per-call
  /// owned-files guard still 409s if files reappear between list and
  /// delete.
  async function confirmBulkRemovePhantoms(): Promise<void> {
    const ids = [...phantomSel.selected];
    if (ids.length === 0) {
      bulkPhantomDeleteOpen = false;
      return;
    }
    bulkPhantomDeleting = true;
    error = null;
    const results = await Promise.allSettled(
      ids.map((id) => deleteSeries(id, { deleteFiles: true }).then(() => id))
    );
    const succeeded = new Set<number>();
    let failed = 0;
    for (const r of results) {
      if (r.status === 'fulfilled') succeeded.add(r.value);
      else failed += 1;
    }
    phantoms = phantoms.filter((p) => !succeeded.has(p.id));
    phantomSel.clear();
    if (failed > 0) {
      toast.warning(
        `Removed ${succeeded.size}; ${failed} failed (files reappeared or path guard tripped).`
      );
    } else {
      toast.success(`Removed ${succeeded.size} series from the catalog.`);
    }
    bulkPhantomDeleting = false;
    bulkPhantomDeleteOpen = false;
  }

  // --- duplicate-pair merge ------------------------------------------
  /// Identify which side of a duplicate pair is the "larger" (merge
  /// target). Tiebreaker on equal owned counts is lower id wins — the
  /// older row is the catalog of record. Returns `{target, source}`
  /// where `target` keeps its identity and `source` is absorbed and
  /// deleted.
  function pickMergeRoles(p: DuplicatePair): {
    target_id: number;
    target_title: string;
    source_id: number;
    source_title: string;
  } {
    const aIsTarget =
      p.a_owned_count > p.b_owned_count ||
      (p.a_owned_count === p.b_owned_count && p.a_id < p.b_id);
    return aIsTarget
      ? {
          target_id: p.a_id,
          target_title: p.a_title,
          source_id: p.b_id,
          source_title: p.b_title
        }
      : {
          target_id: p.b_id,
          target_title: p.b_title,
          source_id: p.a_id,
          source_title: p.a_title
        };
  }

  function pairKey(p: DuplicatePair): string {
    return `${p.a_id}:${p.b_id}`;
  }

  async function handleMergeDuplicate(p: DuplicatePair): Promise<void> {
    const key = pairKey(p);
    if (mergeInFlight.has(key)) return;
    const { target_id, target_title, source_id, source_title } = pickMergeRoles(p);
    mergeInFlight = new Set([...mergeInFlight, key]);
    error = null;
    try {
      await mergeDuplicates(target_id, source_id);
      // Drop the pair locally. Also drop any other pair that references
      // the now-deleted source — the backend would 404 on a stale id
      // anyway, but removing them keeps the UI honest.
      duplicates = duplicates.filter(
        (d) =>
          d.a_id !== source_id &&
          d.b_id !== source_id &&
          // Also drop the just-merged pair itself (covered by the two
          // checks above when the pair has source on a side; explicit
          // here as belt-and-braces).
          pairKey(d) !== key
      );
      toast.success(`Merged ${source_title} into ${target_title}.`);
    } catch (e) {
      const message =
        e instanceof ApiError ? e.message : 'Could not merge the duplicate series.';
      toast.warning(message);
    } finally {
      mergeInFlight = new Set([...mergeInFlight].filter((k) => k !== key));
    }
  }

  // --- folder mutations ----------------------------------------------
  function handleDismissOne(folderName: string): Promise<void> {
    return run(async () => {
      await dismissFolders([folderName]);
      untracked = untracked.filter((f) => f.folder_name !== folderName);
      folderSel.discard(folderName);
      toast.success('Folder dismissed.');
    });
  }

  function handleBulkDismiss(): Promise<void> {
    const names = [...folderSel.selected];
    if (names.length === 0) return Promise.resolve();
    return run(async () => {
      const result = await dismissFolders(names);
      const dismissed = new Set(names);
      untracked = untracked.filter((f) => !dismissed.has(f.folder_name));
      folderSel.clear();
      toast.success(
        `Dismissed ${result.dismissed} folder${result.dismissed === 1 ? '' : 's'}.`
      );
    });
  }

  function handleBulkConvert(): Promise<void> {
    const names = [...folderSel.selected];
    if (names.length === 0) return Promise.resolve();
    return run(async () => {
      const { results } = await convertFolders(names);
      const added = results.filter((r) => r.status === 'added').length;
      const linked = results.filter((r) => r.status === 'linked').length;
      const failed = results.filter((r) => r.status === 'failed').length;
      // Drop every resolved folder — it's attached to a tracked series
      // now, whether the series was new (`added`) or pre-existing
      // (`linked`).
      const done = new Set(
        results
          .filter((r) => r.status === 'added' || r.status === 'linked')
          .map((r) => r.folder_name)
      );
      untracked = untracked.filter((f) => !done.has(f.folder_name));
      folderSel.clear();
      if (failed > 0) {
        toast.warning(`${added} added, ${linked} linked, ${failed} failed.`);
      } else if (added === 0 && linked > 0) {
        toast.success(`Linked ${linked} folder${linked === 1 ? '' : 's'} to existing series.`);
      } else if (linked === 0) {
        toast.success(`Added ${added} folder${added === 1 ? '' : 's'} as tracked series.`);
      } else {
        toast.success(`${added} added, ${linked} linked to existing series.`);
      }
    });
  }

  // --- add modal -----------------------------------------------------
  function openAddModal(folder: DiscoveredFolder): void {
    addModalFolder = folder;
    addCvSelected = null;
    addSubmitting = false;
    addError = null;
  }

  function closeAddModal(): void {
    addModalFolder = null;
    addCvSelected = null;
    addSubmitting = false;
    addError = null;
  }

  // --- enrichment queue ----------------------------------------------
  /** Per-outcome chip styling. The five outcomes map to four colors:
   *  `collision_disabled` and `error` both indicate something the
   *  worker won't retry without user intervention, so they share the
   *  red tint. */
  function outcomeBadgeClasses(outcome: EnrichmentReviewOutcome): string {
    switch (outcome) {
      case 'multi_match':
        return 'border-amber-200 bg-amber-100 text-amber-900';
      case 'low_confidence':
        return 'border-slate-200 bg-slate-100 text-slate-700';
      case 'year_mismatch':
        return 'border-blue-200 bg-blue-100 text-blue-900';
      case 'collision_disabled':
      case 'error':
        return 'border-red-200 bg-red-100 text-red-900';
    }
  }

  function outcomeLabel(outcome: EnrichmentReviewOutcome): string {
    switch (outcome) {
      case 'multi_match':
        return 'Multiple matches';
      case 'low_confidence':
        return 'Low confidence';
      case 'year_mismatch':
        return 'Year mismatch';
      case 'collision_disabled':
        return 'Title collision';
      case 'error':
        return 'Worker error';
    }
  }

  async function handleEnrichmentPick(
    seriesId: number,
    cvId: number,
    seriesTitle: string
  ): Promise<void> {
    if (enrichmentPickInFlight.has(seriesId)) return;
    // Set/Map-replacement on every mutation so Svelte's $state tracks
    // the change — in-place .add()/.delete() on the instance wouldn't
    // trigger reactivity.
    enrichmentPickInFlight = new Set([...enrichmentPickInFlight, seriesId]);
    // Clear any prior collision warning on a fresh attempt.
    if (enrichmentCollisions.has(seriesId)) {
      const next = new Map(enrichmentCollisions);
      next.delete(seriesId);
      enrichmentCollisions = next;
    }
    try {
      await setSeriesCvId(seriesId, cvId);
      // Optimistic local removal — the row is gone from the catalog's
      // shallow set the moment PATCH returns 200, so re-fetching the
      // queue would just confirm what we already know.
      enrichmentQueue = enrichmentQueue.filter((r) => r.id !== seriesId);
      toast.success(`Linked ${seriesTitle} to ComicVine.`);
    } catch (e) {
      if (e instanceof ApiError && e.code === 'conflict.cv_id_in_use') {
        // Stash the collision details on the row instead of toasting —
        // the inline warning gives the user a one-click path to delete
        // the duplicate.
        const d = e.details as
          | { existing_series_id?: number; existing_series_title?: string }
          | null;
        const existingSeriesId = d?.existing_series_id ?? 0;
        const existingSeriesTitle = d?.existing_series_title ?? 'another series';
        enrichmentCollisions = new Map([
          ...enrichmentCollisions,
          [seriesId, { existingSeriesId, existingSeriesTitle }]
        ]);
      } else {
        const message =
          e instanceof ApiError ? e.message : 'Could not set the CV link.';
        toast.warning(message);
      }
    } finally {
      enrichmentPickInFlight = new Set(
        [...enrichmentPickInFlight].filter((id) => id !== seriesId)
      );
    }
  }

  async function handleDeleteDuplicate(
    seriesId: number,
    title: string,
    force: boolean
  ): Promise<void> {
    if (enrichmentDeleteInFlight.has(seriesId)) return;
    enrichmentDeleteInFlight = new Set([...enrichmentDeleteInFlight, seriesId]);
    try {
      await deleteSeries(seriesId, { force });
      enrichmentQueue = enrichmentQueue.filter((r) => r.id !== seriesId);
      const next = new Map(enrichmentCollisions);
      next.delete(seriesId);
      enrichmentCollisions = next;
      // Force-delete unlinks files and kicks off a server-side rescan;
      // the user should know to wait for the rematch to land.
      toast.success(
        force
          ? `Removed duplicate ${title}. Files queued for re-match.`
          : `Removed duplicate ${title}.`
      );
    } catch (e) {
      const message = e instanceof ApiError ? e.message : 'Could not delete the duplicate.';
      toast.warning(message);
    } finally {
      enrichmentDeleteInFlight = new Set(
        [...enrichmentDeleteInFlight].filter((id) => id !== seriesId)
      );
    }
  }

  async function submitAdd(): Promise<void> {
    if (!addModalFolder || !addCvSelected) return;
    const folderName = addModalFolder.folder_name;
    const cvId = addCvSelected.cv_id;
    const cvName = addCvSelected.name;
    addSubmitting = true;
    addError = null;
    try {
      const result = await addFolders([{ folder_name: folderName, cv_id: cvId }]);
      if (result.succeeded.length > 0) {
        untracked = untracked.filter((f) => f.folder_name !== folderName);
        folderSel.discard(folderName);
        closeAddModal();
        toast.success(`Added ${cvName}.`);
      } else {
        // POST /reconcile/add resolves 200 even on a per-row CV failure;
        // surface failed[0] inline and keep the modal open for retry.
        addError = result.failed[0]?.error ?? 'Could not add this folder.';
      }
    } catch (e) {
      addError = e instanceof ApiError ? e.message : String(e);
    } finally {
      addSubmitting = false;
    }
  }
</script>

{#snippet coverThumb(url: string | null)}
  {#if url}
    <img
      src={url}
      alt=""
      class="size-12 flex-shrink-0 rounded bg-slate-100 object-cover"
      loading="lazy"
    />
  {:else}
    <div class="size-12 flex-shrink-0 rounded bg-slate-100" aria-hidden="true"></div>
  {/if}
{/snippet}

<h1 class="mb-1 text-2xl font-bold">Library tidy</h1>
<p class="mb-4 text-sm text-slate-600">
  Reconcile the catalog against what's actually on disk — drop series whose files are gone,
  and fold in series folders LongBox isn't tracking yet.
</p>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

{#if phantoms.length === 0 && kept.length === 0 && untracked.length === 0 && enrichmentQueue.length === 0 && duplicates.length === 0}
  <EmptyState
    icon={Sparkles}
    title="Your library is tidy"
    message="Every tracked series has files on disk, no untracked folders were found, every shallow series is either CV-linked or still inside its enrichment cooldown, and no duplicate-series pairs were detected."
  />
{:else}
  <!-- ===================== Phantom series ===================== -->
  <section class="mb-8">
    <h2 class="mb-3 text-lg font-semibold">Phantom series</h2>

    {#if phantoms.length === 0}
      <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
        No phantom series — every tracked series has files on disk.
      </p>
    {:else}
      <!-- Subsection 1: scheduled for automatic removal — a countdown. -->
      {#if scheduledForRemoval.length > 0}
        <div class="mb-4 rounded-lg border border-red-200 bg-red-50 p-4">
          <h3 class="text-sm font-semibold text-red-900">Scheduled for automatic removal</h3>
          <p class="mb-3 text-xs text-red-800">
            These series have had no files on disk for several scans and will be removed from
            the catalog automatically. Keep one to cancel its removal, or remove it now.
          </p>
          <ul class="space-y-2">
            {#each scheduledForRemoval as p (p.id)}
              <li class="flex items-center gap-3 rounded-md border border-red-200 bg-white p-3">
                {@render coverThumb(p.cover_url)}
                <div class="min-w-0 flex-1">
                  <div class="truncate font-medium" title={p.title}>
                    {p.title}{#if p.start_year}
                      <span class="text-slate-500">({p.start_year})</span>{/if}
                  </div>
                  <div class="text-xs text-red-700">
                    Will be removed on {formatDateTime(p.auto_tidy_due_at)}
                  </div>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleKeep(p.id)}
                  disabled={busy}
                >
                  Keep
                </Button>
                <Button
                  variant="danger"
                  size="sm"
                  onclick={() => requestRemovePhantom(p)}
                  disabled={busy}
                >
                  Remove now
                </Button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      <!-- Subsection 2: transition phantoms — the urgent call to action. -->
      {#if transitionPhantoms.length > 0}
        <div class="mb-4 rounded-lg border border-amber-200 bg-amber-50 p-4">
          <h3 class="text-sm font-semibold text-amber-900">Recently lost files</h3>
          <p class="mb-3 text-xs text-amber-800">
            These series held files at the last scan and have lost them all since — most likely
            you deleted the folder. Remove them from the catalog, or keep them if the files are
            coming back.
          </p>
          <ul class="space-y-2">
            {#each transitionPhantoms as p (p.id)}
              <li
                class="flex items-center gap-3 rounded-md border border-amber-200 bg-white p-3"
              >
                {@render coverThumb(p.cover_url)}
                <div class="min-w-0 flex-1">
                  <div class="truncate font-medium" title={p.title}>
                    {p.title}{#if p.start_year}
                      <span class="text-slate-500">({p.start_year})</span>{/if}
                  </div>
                  <div class="text-xs text-amber-700">
                    Had {p.last_matched_count} matched file{p.last_matched_count === 1
                      ? ''
                      : 's'} at the last scan
                  </div>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleKeep(p.id)}
                  disabled={busy}
                >
                  Keep
                </Button>
                <Button
                  variant="danger"
                  size="sm"
                  onclick={() => requestRemovePhantom(p)}
                  disabled={busy}
                >
                  Remove from catalog
                </Button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      <!-- Subsection 3: steady-state empty-series backlog. -->
      {#if steadyStatePhantoms.length > 0}
        <div class="mb-4">
          <h3 class="mb-2 text-sm font-semibold text-slate-700">Empty series</h3>
          <BulkActionBar
            count={phantomSel.size}
            allSelected={phantomSel.allSelected(steadyIds)}
            someSelected={phantomSel.someSelected(steadyIds)}
            onToggleAll={() => phantomSel.toggleAll(steadyIds)}
            selectAllLabel="Select all empty series"
          >
            {#snippet action()}
              <Button
                variant="danger"
                size="sm"
                onclick={requestBulkRemovePhantoms}
                disabled={busy || phantomSel.size === 0}
              >
                Remove selected
              </Button>
            {/snippet}
          </BulkActionBar>
          <ul class="space-y-2">
            {#each steadyStatePhantoms as p (p.id)}
              <li
                class="flex items-center gap-3 rounded-md border border-slate-200 bg-white p-3"
              >
                <input
                  type="checkbox"
                  class="rounded border-slate-300"
                  checked={phantomSel.has(p.id)}
                  onchange={() => phantomSel.toggle(p.id)}
                  aria-label={`Select ${p.title}`}
                />
                {@render coverThumb(p.cover_url)}
                <div class="min-w-0 flex-1">
                  <div class="truncate font-medium" title={p.title}>
                    {p.title}{#if p.start_year}
                      <span class="text-slate-500">({p.start_year})</span>{/if}
                  </div>
                  {#if p.publisher}
                    <div class="truncate text-xs text-slate-500">{p.publisher}</div>
                  {/if}
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => requestRemovePhantom(p)}
                  disabled={busy}
                >
                  Remove
                </Button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      <!-- Subsection 4: awaiting first download — informational, no action needed. -->
      {#if awaitingFirstDownload.length > 0}
        <div>
          <h3 class="mb-1 text-sm font-semibold text-slate-700">Awaiting first download</h3>
          <p class="mb-2 text-xs text-slate-500">
            On the pull list but not yet downloaded — these aren't a problem; LongBox fills
            them in as issues are grabbed.
          </p>
          <ul class="space-y-2">
            {#each awaitingFirstDownload as p (p.id)}
              <li
                class="flex items-center gap-3 rounded-md border border-slate-200 bg-slate-50 p-3"
              >
                {@render coverThumb(p.cover_url)}
                <div class="min-w-0 flex-1">
                  <div class="truncate font-medium" title={p.title}>
                    {p.title}{#if p.start_year}
                      <span class="text-slate-500">({p.start_year})</span>{/if}
                  </div>
                  {#if p.publisher}
                    <div class="truncate text-xs text-slate-500">{p.publisher}</div>
                  {/if}
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => requestRemovePhantom(p)}
                  disabled={busy}
                >
                  Remove
                </Button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    {/if}
  </section>

  <!-- ==================== Duplicate series ==================== -->
  <section class="mb-8">
    <h2 class="mb-1 text-lg font-semibold">Duplicate series</h2>
    <p class="mb-2 text-xs text-slate-500">
      Pairs that look like the same series under two catalog rows — either both linking to the
      same ComicVine volume, or sharing a normalized title with start years within 2 of each
      other. "Merge" moves every issue (and its files) from the smaller side into the larger,
      then deletes the smaller row. The schema's <code class="font-mono">UNIQUE(cv_issue_id)</code>
      keeps merges safe — collisions are structurally impossible.
    </p>
    {#if duplicates.length === 0}
      <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
        No duplicates detected.
      </p>
    {:else}
      <ul class="space-y-2">
        {#each duplicates as p (pairKey(p))}
          {@const roles = pickMergeRoles(p)}
          {@const isMerging = mergeInFlight.has(pairKey(p))}
          <li class="rounded-lg border border-slate-200 bg-white p-3">
            <div class="mb-2 flex items-baseline justify-between gap-2">
              <span class="text-xs uppercase tracking-wide text-slate-500">
                {p.kind === 'same_cv_id' ? 'Same ComicVine volume' : 'Same title, close year'}
              </span>
              <Button
                variant="secondary"
                size="sm"
                loading={isMerging}
                disabled={isMerging}
                onclick={() => handleMergeDuplicate(p)}
              >
                Merge into {roles.target_title}
              </Button>
            </div>
            <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
              <div class="rounded border border-slate-100 bg-slate-50 p-2 text-sm">
                <div class="font-medium">
                  <a class="text-blue-600 hover:underline" href={`/series/${p.a_id}`}>
                    {p.a_title}{p.a_start_year ? ` (${p.a_start_year})` : ''}
                  </a>
                  {#if roles.target_id === p.a_id}
                    <span class="ml-1 rounded bg-emerald-100 px-1.5 py-0.5 text-xs text-emerald-700">
                      target
                    </span>
                  {/if}
                </div>
                <div class="text-xs text-slate-500">
                  <span class="font-mono">#{p.a_id}</span>
                  · cv_id: <span class="font-mono">{p.a_cv_id ?? '—'}</span>
                  · {p.a_owned_count} owned
                </div>
              </div>
              <div class="rounded border border-slate-100 bg-slate-50 p-2 text-sm">
                <div class="font-medium">
                  <a class="text-blue-600 hover:underline" href={`/series/${p.b_id}`}>
                    {p.b_title}{p.b_start_year ? ` (${p.b_start_year})` : ''}
                  </a>
                  {#if roles.target_id === p.b_id}
                    <span class="ml-1 rounded bg-emerald-100 px-1.5 py-0.5 text-xs text-emerald-700">
                      target
                    </span>
                  {/if}
                </div>
                <div class="text-xs text-slate-500">
                  <span class="font-mono">#{p.b_id}</span>
                  · cv_id: <span class="font-mono">{p.b_cv_id ?? '—'}</span>
                  · {p.b_owned_count} owned
                </div>
              </div>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <!-- ==================== Untracked folders ==================== -->
  <section>
    <h2 class="mb-3 text-lg font-semibold">Untracked folders</h2>
    {#if untracked.length === 0}
      <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
        No untracked folders — every series folder on disk is in the catalog.
      </p>
    {:else}
      <p class="mb-2 text-xs text-slate-500">
        "Convert" tracks a folder as a series immediately, with issues read from its
        filenames — no ComicVine. Covers and full metadata fill in later.
      </p>
      <BulkActionBar
        count={folderSel.size}
        allSelected={folderSel.allSelected(untrackedNames)}
        someSelected={folderSel.someSelected(untrackedNames)}
        onToggleAll={() => folderSel.toggleAll(untrackedNames)}
        selectAllLabel="Select all untracked folders"
      >
        {#snippet action()}
          <div class="flex gap-2">
            <Button
              size="sm"
              onclick={handleBulkConvert}
              disabled={busy || folderSel.size === 0}
            >
              {folderSel.size > 0 ? `Convert ${folderSel.size} selected` : 'Convert selected'}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onclick={handleBulkDismiss}
              disabled={busy || folderSel.size === 0}
            >
              Dismiss selected
            </Button>
          </div>
        {/snippet}
      </BulkActionBar>
      <ul class="space-y-2">
        {#each untracked as f (f.folder_name)}
          <li class="flex items-center gap-3 rounded-md border border-slate-200 bg-white p-3">
            <input
              type="checkbox"
              class="rounded border-slate-300"
              checked={folderSel.has(f.folder_name)}
              onchange={() => folderSel.toggle(f.folder_name)}
              aria-label={`Select ${f.folder_name}`}
            />
            <Folder class="size-5 flex-shrink-0 text-slate-400" aria-hidden="true" />
            <div class="min-w-0 flex-1">
              <div class="truncate font-medium" title={f.folder_name}>{f.folder_name}</div>
              <div class="text-xs text-slate-500">
                {f.file_count} file{f.file_count === 1 ? '' : 's'}
              </div>
            </div>
            <Button size="sm" onclick={() => openAddModal(f)} disabled={busy}>
              Add to LongBox
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onclick={() => handleDismissOne(f.folder_name)}
              disabled={busy}
            >
              Dismiss
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <!-- ================= Enrichment needs review ================= -->
  {#if enrichmentQueue.length > 0}
    <section class="mt-8">
      <h2 class="mb-1 text-lg font-semibold">Enrichment needs review</h2>
      <p class="mb-3 text-xs text-slate-500">
        {enrichmentQueue.length}
        series need{enrichmentQueue.length === 1 ? 's' : ''} a ComicVine pick — the auto-link
        worker found a match it couldn't accept on its own. Search ComicVine for each row and
        choose the right volume.
      </p>
      <ul class="space-y-2">
        {#each enrichmentQueue as row (row.id)}
          <li class="rounded-md border border-slate-200 bg-white p-3">
            <div class="flex items-start gap-3">
              <div class="min-w-0 flex-1">
                <div class="truncate font-medium" title={row.title}>
                  {row.title}{#if row.start_year}
                    <span class="text-slate-500">({row.start_year})</span>{/if}
                </div>
                <div class="mt-1 flex flex-wrap items-center gap-2 text-xs">
                  <span
                    class="inline-flex items-center rounded-full border px-2 py-0.5 text-[11px] font-medium {outcomeBadgeClasses(
                      row.last_enrichment_outcome
                    )}"
                    title={row.last_enrichment_error ?? undefined}
                  >
                    {outcomeLabel(row.last_enrichment_outcome)}
                  </span>
                  <span class="text-slate-500">
                    {row.owned_count} owned file{row.owned_count === 1 ? '' : 's'}
                  </span>
                </div>
              </div>
            </div>
            <div class="mt-3">
              <CvSearchInput
                onSelect={(r) => handleEnrichmentPick(row.id, r.cv_id, row.title)}
                disabled={enrichmentPickInFlight.has(row.id)}
              />
            </div>
            {#if enrichmentCollisions.has(row.id)}
              {@const collision = enrichmentCollisions.get(row.id)!}
              <div
                class="mt-3 flex items-start gap-3 rounded-md border border-amber-200 bg-amber-50 p-3"
              >
                <div class="min-w-0 flex-1 text-sm">
                  <div class="font-medium text-amber-900">
                    Already linked to "{collision.existingSeriesTitle}"
                  </div>
                  {#if row.owned_count === 0}
                    <div class="text-xs text-amber-800">
                      This looks like a duplicate of an existing series. Delete it?
                    </div>
                  {:else}
                    <div class="text-xs text-amber-800">
                      This series has {row.owned_count} owned file{row.owned_count === 1
                        ? ''
                        : 's'}. They're almost certainly misassigned — physically
                      they live under "{collision.existingSeriesTitle}" and will
                      re-match automatically after the duplicate is removed.
                    </div>
                  {/if}
                </div>
                {#if row.owned_count === 0}
                  <Button
                    variant="danger"
                    size="sm"
                    onclick={() => handleDeleteDuplicate(row.id, row.title, false)}
                    disabled={enrichmentDeleteInFlight.has(row.id)}
                  >
                    Delete duplicate
                  </Button>
                {:else}
                  <Button
                    variant="danger"
                    size="sm"
                    onclick={() => handleDeleteDuplicate(row.id, row.title, true)}
                    disabled={enrichmentDeleteInFlight.has(row.id)}
                  >
                    Delete duplicate anyway
                  </Button>
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <!-- ====================== Kept series ======================= -->
  <!-- Collapsed by default — low-traffic management surface for the
       series the user has exempted from tidy. Native <details> does the
       collapse; no JS state needed. -->
  {#if kept.length > 0}
    <details class="mt-8 rounded-lg border border-slate-200 bg-white">
      <summary class="cursor-pointer select-none px-4 py-3 text-lg font-semibold">
        Kept series
        <span class="ml-1 text-sm font-normal text-slate-500">({kept.length})</span>
      </summary>
      <div class="border-t border-slate-100 px-4 py-3">
        <p class="mb-3 text-xs text-slate-500">
          These series are exempt from Library Tidy — they won't be flagged as phantoms or
          auto-removed, even with no files on disk. Un-keep one to let LongBox evaluate it
          normally again.
        </p>
        <ul class="space-y-2">
          {#each kept as p (p.id)}
            <li class="flex items-center gap-3 rounded-md border border-slate-200 bg-slate-50 p-3">
              {@render coverThumb(p.cover_url)}
              <div class="min-w-0 flex-1">
                <div class="truncate font-medium" title={p.title}>
                  {p.title}{#if p.start_year}
                    <span class="text-slate-500">({p.start_year})</span>{/if}
                </div>
                {#if p.publisher}
                  <div class="truncate text-xs text-slate-500">{p.publisher}</div>
                {/if}
              </div>
              <Button
                variant="ghost"
                size="sm"
                onclick={() => handleUnkeep(p.id)}
                disabled={busy}
              >
                Un-keep
              </Button>
            </li>
          {/each}
        </ul>
      </div>
    </details>
  {/if}
{/if}

<Modal open={addModalFolder !== null} title="Add to LongBox" onClose={closeAddModal}>
  {#if addModalFolder}
    <p class="mb-1 text-xs text-slate-500">Folder</p>
    <p class="mb-4 truncate font-mono text-sm" title={addModalFolder.folder_name}>
      {addModalFolder.folder_name}
    </p>

    {#if addError}
      <p class="mb-3 rounded-md bg-red-50 px-3 py-2 text-sm text-red-700">{addError}</p>
    {/if}

    {#if addCvSelected}
      <div class="mb-3 rounded-md border border-slate-200 bg-slate-50 p-3 text-sm">
        <div class="font-medium">
          {addCvSelected.name}{#if addCvSelected.start_year}
            <span class="text-slate-500">({addCvSelected.start_year})</span>{/if}
        </div>
        <div class="text-xs text-slate-500">
          {addCvSelected.publisher ?? 'Unknown publisher'} ·
          {addCvSelected.issue_count} issue{addCvSelected.issue_count === 1 ? '' : 's'}
        </div>
        <button
          type="button"
          class="mt-1 text-xs text-blue-700 hover:underline"
          onclick={() => {
            addCvSelected = null;
            addError = null;
          }}
        >
          Change selection
        </button>
      </div>
      <div class="flex justify-end gap-2">
        <Button variant="ghost" onclick={closeAddModal}>Cancel</Button>
        <Button onclick={submitAdd} loading={addSubmitting}>Add</Button>
      </div>
    {:else}
      <CvSearchInput
        initialQuery={addHint}
        onSelect={(r) => {
          addCvSelected = r;
          addError = null;
        }}
      />
      <p class="mt-3 text-xs text-slate-500">
        Adding the series fetches it from ComicVine and queues a rematch so the folder's files
        attach to it.
      </p>
    {/if}
  {/if}
</Modal>

<!-- Per-row phantom delete confirmation. Open only when a target is
     set; never auto-opens. Adapts copy based on last_matched_count so
     a transition phantom (recently lost files) reads differently from
     a never-had-files steady-state row. -->
{#if phantomDeleteTarget}
  {@const target = phantomDeleteTarget}
  {@const folder = folderNameFor(target)}
  <Modal open={true} title="Delete series?" onClose={() => (phantomDeleteTarget = null)}>
    <p class="text-sm">
      Remove
      <strong>{target.title}{target.start_year ? ` (${target.start_year})` : ''}</strong>
      from the catalog.
      {#if target.last_matched_count > 0}
        It had <strong>{target.last_matched_count} file{target.last_matched_count === 1 ? '' : 's'}</strong>
        linked at the last scan — those files are no longer on disk.
      {:else}
        It currently has no files linked.
      {/if}
    </p>
    <p class="mt-2 text-sm">
      Folder to be removed (if it still exists):
      <code class="ml-1 break-all rounded bg-slate-100 px-1.5 py-0.5 font-mono text-xs">
        {folder}/
      </code>
    </p>
    <p class="mt-2 text-xs text-slate-500">
      If the folder is absent (already cleaned up, never existed), the catalog entry is removed
      and a warning is logged server-side. <strong>This cannot be undone.</strong>
    </p>
    <div class="mt-4 flex justify-end gap-2">
      <Button
        variant="ghost"
        onclick={() => (phantomDeleteTarget = null)}
        disabled={phantomDeleting}
      >
        Cancel
      </Button>
      <Button variant="danger" onclick={confirmRemovePhantom} loading={phantomDeleting}>
        Delete series
      </Button>
    </div>
  </Modal>
{/if}

<!-- Bulk phantom delete confirmation. Renders the count + a short
     preview list so the user can sanity-check before firing N
     destructive calls. The preview caps at 8 to keep the modal small;
     the count is the load-bearing detail anyway. -->
{#if bulkPhantomDeleteOpen}
  {@const ids = [...phantomSel.selected]}
  {@const selectedPhantoms = phantoms.filter((p) => phantomSel.selected.has(p.id))}
  {@const previewCount = 8}
  <Modal
    open={true}
    title={`Delete ${ids.length} series?`}
    onClose={() => (bulkPhantomDeleteOpen = false)}
  >
    <p class="text-sm">
      Remove <strong>{ids.length}</strong> series from the catalog. For each one, the on-disk
      folder will also be removed if it still exists.
      <strong>This cannot be undone.</strong>
    </p>
    {#if selectedPhantoms.length > 0}
      <ul class="mt-3 max-h-48 overflow-y-auto rounded border border-slate-200 bg-slate-50 p-2 text-xs">
        {#each selectedPhantoms.slice(0, previewCount) as p (p.id)}
          <li class="py-0.5 font-mono">
            {folderNameFor(p)}/
          </li>
        {/each}
        {#if selectedPhantoms.length > previewCount}
          <li class="py-0.5 text-slate-500">
            … and {selectedPhantoms.length - previewCount} more
          </li>
        {/if}
      </ul>
    {/if}
    <div class="mt-4 flex justify-end gap-2">
      <Button
        variant="ghost"
        onclick={() => (bulkPhantomDeleteOpen = false)}
        disabled={bulkPhantomDeleting}
      >
        Cancel
      </Button>
      <Button
        variant="danger"
        onclick={confirmBulkRemovePhantoms}
        loading={bulkPhantomDeleting}
      >
        Delete {ids.length} series
      </Button>
    </div>
  </Modal>
{/if}
