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
    addFolders,
    bulkDeletePhantoms,
    convertFolders,
    deletePhantom,
    dismissFolders,
    keepPhantom,
    type DiscoveredFolder,
    type PhantomSeries
  } from '$lib/api/reconcile';
  import { createSelection } from '$lib/createSelection.svelte';
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
  let untracked = $state<DiscoveredFolder[]>([...data.untracked]);

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

  // The /reconcile/phantoms API returns `with_transition` and
  // `all_zero_owned` as OVERLAPPING lists — a transition phantom appears
  // in both, which is correct for the API (`all_zero_owned` means
  // literally "every zero-owned series"). The UI deliberately renders
  // the two subsections as a DISJOINT partition instead: a transition
  // phantom shows ONLY under "Recently lost files", never also under
  // "Zero ownership". Deriving both subsets from the single `phantoms`
  // list here — rather than tracking the API's two lists separately — is
  // what makes that work, and it makes "Keep" a pure local mutation: set
  // last_matched_count to 0 and the row reactively slides from one
  // subsection to the other with no refetch. Do not "fix" this back into
  // rendering the API's overlapping lists directly — the disjoint UI is
  // intentional.
  const transitionPhantoms = $derived(phantoms.filter((p) => p.last_matched_count > 0));
  const steadyStatePhantoms = $derived(phantoms.filter((p) => p.last_matched_count === 0));

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
      // Local last_matched_count -> 0; the $derived split moves the row
      // from "Recently lost files" down to "Zero ownership".
      phantoms = phantoms.map((p) =>
        p.id === seriesId ? { ...p, last_matched_count: 0 } : p
      );
      toast.success('Kept — moved to Zero ownership.');
    });
  }

  function handleRemovePhantom(seriesId: number): Promise<void> {
    return run(async () => {
      await deletePhantom(seriesId);
      phantoms = phantoms.filter((p) => p.id !== seriesId);
      phantomSel.discard(seriesId);
      toast.success('Series removed from the catalog.');
    });
  }

  function handleBulkRemovePhantoms(): Promise<void> {
    const ids = [...phantomSel.selected];
    if (ids.length === 0) return Promise.resolve();
    return run(async () => {
      const result = await bulkDeletePhantoms(ids);
      const deleted = new Set(result.deleted);
      phantoms = phantoms.filter((p) => !deleted.has(p.id));
      phantomSel.clear();
      if (result.skipped.length > 0) {
        toast.warning(
          `Removed ${result.deleted.length}; skipped ${result.skipped.length} (files reappeared).`
        );
      } else {
        toast.success(`Removed ${result.deleted.length} series from the catalog.`);
      }
    });
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
      const converted = results.filter((r) => r.status === 'converted').length;
      const failed = results.filter((r) => r.status === 'failed').length;
      // Drop every converted folder — it's a tracked series now.
      const done = new Set(
        results.filter((r) => r.status === 'converted').map((r) => r.folder_name)
      );
      untracked = untracked.filter((f) => !done.has(f.folder_name));
      folderSel.clear();
      if (failed > 0) {
        toast.warning(`Converted ${converted}; ${failed} failed.`);
      } else {
        toast.success(
          `Converted ${converted} folder${converted === 1 ? '' : 's'} to tracked series.`
        );
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

{#if phantoms.length === 0 && untracked.length === 0}
  <EmptyState
    icon={Sparkles}
    title="Your library is tidy"
    message="Every tracked series has files on disk, and no untracked folders were found."
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
      <!-- Subsection 1: transition phantoms — the urgent call to action. -->
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
                  onclick={() => handleRemovePhantom(p.id)}
                  disabled={busy}
                >
                  Remove from catalog
                </Button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      <!-- Subsection 2: steady-state zero-ownership backlog. -->
      <div>
        <h3 class="mb-2 text-sm font-semibold text-slate-700">Zero ownership</h3>
        {#if steadyStatePhantoms.length === 0}
          <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
            Nothing here{#if transitionPhantoms.length > 0}
              — every phantom is in "Recently lost files" above{/if}.
          </p>
        {:else}
          <BulkActionBar
            count={phantomSel.size}
            allSelected={phantomSel.allSelected(steadyIds)}
            someSelected={phantomSel.someSelected(steadyIds)}
            onToggleAll={() => phantomSel.toggleAll(steadyIds)}
            selectAllLabel="Select all zero-ownership series"
          >
            {#snippet action()}
              <Button
                variant="danger"
                size="sm"
                onclick={handleBulkRemovePhantoms}
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
                  onclick={() => handleRemovePhantom(p.id)}
                  disabled={busy}
                >
                  Remove
                </Button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
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
