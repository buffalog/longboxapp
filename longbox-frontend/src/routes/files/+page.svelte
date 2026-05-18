<script lang="ts">
  import { goto, invalidate } from '$app/navigation';
  import { Files, LayoutGrid, List } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    clearFileIgnored,
    markFileIgnored,
    matchFileFromCv,
    matchFolderFromCv,
    setFileIssue,
    type FolderMatchResponse
  } from '$lib/api/files';
  import Button from '$lib/components/Button.svelte';
  import CvSearchInput from '$lib/components/CvSearchInput.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import FileRow from '$lib/components/FileRow.svelte';
  import FolderCard from '$lib/components/FolderCard.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import { searchHintFromPath } from '$lib/match_hint';
  import type { EnrichedFileRow, FileStatus, SeriesSearchResult } from '$lib/types';

  let { data } = $props();

  let error = $state<ApiError | null>(null);
  let busyId = $state<number | null>(null);
  let pendingChange = $state<EnrichedFileRow | null>(null);
  let mode = $state<'cv' | 'issue_id'>('cv');
  let issueIdInput = $state('');
  let cvSelected = $state<SeriesSearchResult | null>(null);
  let cvIssueNumberInput = $state('');
  let cvSubmitting = $state(false);

  // Folder-grouped view state. `view` and `folderFilter` are URL-synced
  // (?view=flat|folder, ?folder_filter=…) so a shared link reproduces the
  // sender's exact view. Filter only applies in folder view.
  let view = $state<'flat' | 'folder'>(data.view);
  let folderFilter = $state<string>(data.folderFilter);
  let folderModalFolder = $state<string | null>(null);
  let folderCvSelected = $state<SeriesSearchResult | null>(null);
  let folderSubmitting = $state(false);
  let folderBusyFolder = $state<string | null>(null);
  let lastFolderResult = $state<{ folder: string; result: FolderMatchResponse } | null>(null);

  const statusOptions: Array<{ value: FileStatus | 'all'; label: string }> = [
    { value: 'needs_review', label: 'Needs review' },
    { value: 'unmatched', label: 'Unmatched' },
    { value: 'ignored', label: 'Ignored' },
    { value: 'owned', label: 'Owned' },
    { value: 'all', label: 'All' }
  ];

  // Folder grouping has actionable value only on filters where files can
  // be bulk-matched. On owned/ignored/all it would just be visual chrome
  // with no useful action; keep the toggle hidden there.
  const folderViewAllowed = $derived(
    data.status === 'unmatched' || data.status === 'needs_review'
  );
  const effectiveView = $derived(folderViewAllowed ? view : 'flat');

  const allFolderGroups = $derived.by(() => {
    if (effectiveView !== 'folder') return [];
    const m = new Map<string, EnrichedFileRow[]>();
    for (const f of data.files) {
      const i = f.path_relative.lastIndexOf('/');
      const dir = i === -1 ? '' : f.path_relative.slice(0, i);
      const list = m.get(dir) ?? [];
      list.push(f);
      m.set(dir, list);
    }
    return Array.from(m.entries())
      .map(([folder, files]) => ({ folder, files, count: files.length }))
      .sort((a, b) => a.folder.localeCompare(b.folder));
  });

  const folderGroups = $derived.by(() => {
    const q = folderFilter.trim().toLowerCase();
    if (!q) return allFolderGroups;
    return allFolderGroups.filter((g) => g.folder.toLowerCase().includes(q));
  });

  const initialHint = $derived(
    pendingChange ? searchHintFromPath(pendingChange.path_relative) : ''
  );
  const folderHint = $derived(
    folderModalFolder ? searchHintFromPath(`${folderModalFolder}/_.cbz`) : ''
  );
  const folderActionableCount = $derived.by(() => {
    if (!folderModalFolder) return 0;
    return folderGroups.find((g) => g.folder === folderModalFolder)?.count ?? 0;
  });

  function setStatus(s: FileStatus | 'all'): void {
    const url = new URL(window.location.href);
    url.searchParams.set('status', s);
    void goto(url.pathname + url.search, { replaceState: true });
  }

  function setView(next: 'flat' | 'folder'): void {
    if (next === view) return;
    view = next;
    const url = new URL(window.location.href);
    if (next === 'folder') {
      url.searchParams.set('view', 'folder');
      // Honor any folder_filter already in the URL (shared link).
      // Otherwise start with an empty filter.
      const pasted = url.searchParams.get('folder_filter') ?? '';
      folderFilter = pasted;
    } else {
      url.searchParams.delete('view');
      url.searchParams.delete('folder_filter');
      folderFilter = '';
    }
    void goto(url.pathname + url.search, { replaceState: true });
  }

  // Filter input updates URL via history.replaceState rather than goto(),
  // because goto() would re-run load() (and re-fire listFiles) on every
  // keystroke. The data is already loaded; folder filtering is purely a
  // $derived transformation of allFolderGroups.
  function setFolderFilter(next: string): void {
    folderFilter = next;
    const url = new URL(window.location.href);
    if (next.trim() === '') {
      url.searchParams.delete('folder_filter');
    } else {
      url.searchParams.set('folder_filter', next);
    }
    window.history.replaceState(null, '', url.pathname + url.search);
  }

  async function withBusy(id: number, fn: () => Promise<void>): Promise<void> {
    busyId = id;
    error = null;
    try {
      await fn();
      await invalidate(() => true);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      busyId = null;
    }
  }

  function handleAccept(file: EnrichedFileRow): Promise<void> {
    if (!file.issue) {
      error = new ApiError(0, 'no_match', 'No suggested match to accept');
      return Promise.resolve();
    }
    const issueId = file.issue.id;
    return withBusy(file.id, async () => {
      await setFileIssue(file.id, issueId);
    });
  }

  function handleIgnore(file: EnrichedFileRow): Promise<void> {
    return withBusy(file.id, async () => {
      await markFileIgnored(file.id);
    });
  }

  function handleClearIgnore(file: EnrichedFileRow): Promise<void> {
    return withBusy(file.id, async () => {
      await clearFileIgnored(file.id);
    });
  }

  function openChange(file: EnrichedFileRow): void {
    pendingChange = file;
    mode = 'cv';
    issueIdInput = file.issue?.id ? String(file.issue.id) : '';
    cvSelected = null;
    cvIssueNumberInput = '';
    cvSubmitting = false;
  }

  function closeChange(): void {
    pendingChange = null;
    issueIdInput = '';
    cvSelected = null;
    cvIssueNumberInput = '';
    cvSubmitting = false;
  }

  async function submitIssueId(): Promise<void> {
    if (!pendingChange) return;
    const issueId = Number(issueIdInput);
    if (!Number.isFinite(issueId) || issueId <= 0) {
      error = new ApiError(400, 'bad_input', 'Enter a valid issue id');
      return;
    }
    const fileId = pendingChange.id;
    closeChange();
    await withBusy(fileId, async () => {
      await setFileIssue(fileId, issueId);
    });
  }

  async function submitCvMatch(): Promise<void> {
    if (!pendingChange || !cvSelected) return;
    const fileId = pendingChange.id;
    const cvVolumeId = cvSelected.cv_id;
    const issueNumber = cvIssueNumberInput.trim() || undefined;

    cvSubmitting = true;
    error = null;
    try {
      await matchFileFromCv(fileId, cvVolumeId, issueNumber);
      closeChange();
      await invalidate(() => true);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      cvSubmitting = false;
    }
  }

  function openFolderMatch(folder: string): void {
    folderModalFolder = folder;
    folderCvSelected = null;
    folderSubmitting = false;
  }

  function closeFolderMatch(): void {
    folderModalFolder = null;
    folderCvSelected = null;
    folderSubmitting = false;
  }

  async function submitFolderMatch(): Promise<void> {
    if (!folderModalFolder || !folderCvSelected) return;
    const folder = folderModalFolder;
    const cvVolumeId = folderCvSelected.cv_id;

    folderSubmitting = true;
    folderBusyFolder = folder;
    error = null;
    try {
      const result = await matchFolderFromCv(folder, cvVolumeId);
      lastFolderResult = { folder, result };
      closeFolderMatch();
      await invalidate(() => true);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      folderSubmitting = false;
      folderBusyFolder = null;
    }
  }
</script>

<h1 class="mb-4 text-2xl font-bold">Files</h1>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

{#if lastFolderResult}
  <div
    class="mb-4 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-sm"
    role="status"
  >
    <div class="flex items-start justify-between gap-3">
      <div class="min-w-0">
        <div class="font-medium text-emerald-900">
          Matched {lastFolderResult.result.matched_count}
          file{lastFolderResult.result.matched_count === 1 ? '' : 's'} in
          <span class="font-mono">{lastFolderResult.folder}</span>
        </div>
        {#if lastFolderResult.result.skipped.length > 0}
          <details class="mt-1 text-emerald-800">
            <summary class="cursor-pointer text-xs">
              Skipped {lastFolderResult.result.skipped.length} —
              click for details
            </summary>
            <ul class="mt-1 max-h-40 space-y-0.5 overflow-y-auto text-xs">
              {#each lastFolderResult.result.skipped as s (s.path)}
                <li class="font-mono">
                  <span class="text-emerald-600">[{s.reason}]</span> {s.path}
                </li>
              {/each}
            </ul>
          </details>
        {/if}
      </div>
      <button
        type="button"
        class="rounded p-1 text-emerald-700 hover:bg-emerald-100"
        onclick={() => (lastFolderResult = null)}
        aria-label="Dismiss"
      >×</button>
    </div>
  </div>
{/if}

<div class="mb-4 flex flex-wrap items-center gap-2">
  <div class="flex flex-wrap gap-2" role="tablist" aria-label="File status filter">
    {#each statusOptions as opt (opt.value)}
      <button
        type="button"
        role="tab"
        aria-selected={data.status === opt.value}
        onclick={() => setStatus(opt.value)}
        class="rounded-full border px-3 py-1 text-sm transition"
        class:border-slate-900={data.status === opt.value}
        class:bg-slate-900={data.status === opt.value}
        class:text-white={data.status === opt.value}
        class:border-slate-300={data.status !== opt.value}
        class:bg-white={data.status !== opt.value}
        class:hover:bg-slate-50={data.status !== opt.value}
      >
        {opt.label}
      </button>
    {/each}
  </div>

  {#if folderViewAllowed}
    <div
      class="ml-auto inline-flex rounded-md border border-slate-300 bg-white p-0.5 text-sm"
      role="tablist"
      aria-label="View mode"
    >
      <button
        type="button"
        role="tab"
        aria-selected={view === 'flat'}
        onclick={() => setView('flat')}
        class="inline-flex items-center gap-1 rounded px-2.5 py-1 transition"
        class:bg-slate-900={view === 'flat'}
        class:text-white={view === 'flat'}
        class:text-slate-600={view !== 'flat'}
        class:hover:bg-slate-50={view !== 'flat'}
      >
        <List class="size-3.5" aria-hidden="true" />Flat
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={view === 'folder'}
        onclick={() => setView('folder')}
        class="inline-flex items-center gap-1 rounded px-2.5 py-1 transition"
        class:bg-slate-900={view === 'folder'}
        class:text-white={view === 'folder'}
        class:text-slate-600={view !== 'folder'}
        class:hover:bg-slate-50={view !== 'folder'}
      >
        <LayoutGrid class="size-3.5" aria-hidden="true" />By folder
      </button>
    </div>
  {/if}
</div>

{#if folderViewAllowed && effectiveView === 'folder'}
  <div class="mb-4">
    <div class="relative max-w-md">
      <input
        type="search"
        placeholder="Filter folders…"
        value={folderFilter}
        oninput={(e) => setFolderFilter((e.target as HTMLInputElement).value)}
        class="w-full rounded-md border border-slate-300 py-1.5 pl-3 pr-8 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        aria-label="Filter folders by name"
      />
      {#if folderFilter !== ''}
        <button
          type="button"
          onclick={() => setFolderFilter('')}
          aria-label="Clear folder filter"
          class="absolute right-2 top-1/2 -translate-y-1/2 rounded p-0.5 text-slate-400 hover:text-slate-700 focus:outline-none focus:ring-2 focus:ring-blue-500"
        >×</button>
      {/if}
    </div>
    <p class="mt-1 text-xs text-slate-500">
      Showing {folderGroups.length} of {allFolderGroups.length} folder{allFolderGroups.length === 1 ? '' : 's'}
    </p>
  </div>
{/if}

{#if data.files.length === 0}
  <EmptyState
    icon={Files}
    title={data.status === 'needs_review' ? 'Nothing needs review' : 'No files in this view'}
    message="Run a scan to populate the catalog, or pick a different filter."
  />
{:else if effectiveView === 'folder'}
  {#if folderGroups.length === 0}
    <div class="rounded-lg border border-slate-200 bg-white p-6 text-center text-sm text-slate-500">
      No folders match
      <span class="font-mono text-slate-700">"{folderFilter}"</span>.
      <button
        type="button"
        class="ml-1 text-blue-600 hover:underline"
        onclick={() => setFolderFilter('')}
      >Clear filter</button>
    </div>
  {:else}
    <ul class="space-y-2">
      {#each folderGroups as g (g.folder)}
        <li>
          <FolderCard
            folder={g.folder || '(library root)'}
            count={g.count}
            busy={folderBusyFolder === g.folder}
            onOpen={() => openFolderMatch(g.folder)}
          />
        </li>
      {/each}
    </ul>
  {/if}
{:else}
  <ul class="space-y-3">
    {#each data.files as file (file.id)}
      <li>
        <FileRow
          {file}
          busy={busyId === file.id}
          onAccept={handleAccept}
          onIgnore={handleIgnore}
          onClearIgnore={handleClearIgnore}
          onChange={openChange}
        />
      </li>
    {/each}
  </ul>
{/if}

<Modal open={!!pendingChange} title="Change match" onClose={closeChange}>
  {#if pendingChange}
    <p class="mb-3 truncate text-xs text-slate-500" title={pendingChange.path_relative}>
      {pendingChange.path_relative}
    </p>

    <div class="mb-4 inline-flex rounded-md border border-slate-200 p-0.5 text-sm">
      <button
        type="button"
        onclick={() => (mode = 'cv')}
        class="rounded px-3 py-1 transition"
        class:bg-slate-900={mode === 'cv'}
        class:text-white={mode === 'cv'}
        class:text-slate-700={mode !== 'cv'}
        class:hover:bg-slate-100={mode !== 'cv'}
      >
        Search ComicVine
      </button>
      <button
        type="button"
        onclick={() => (mode = 'issue_id')}
        class="rounded px-3 py-1 transition"
        class:bg-slate-900={mode === 'issue_id'}
        class:text-white={mode === 'issue_id'}
        class:text-slate-700={mode !== 'issue_id'}
        class:hover:bg-slate-100={mode !== 'issue_id'}
      >
        By issue id
      </button>
    </div>

    {#if mode === 'cv'}
      {#if cvSelected}
        <div class="mb-3 rounded-md border border-slate-200 bg-slate-50 p-3 text-sm">
          <div class="font-medium">
            {cvSelected.name}{#if cvSelected.start_year} <span class="text-slate-500">({cvSelected.start_year})</span>{/if}
          </div>
          <div class="text-xs text-slate-500">{cvSelected.publisher ?? 'Unknown publisher'}</div>
          <button
            type="button"
            class="mt-1 text-xs text-blue-700 hover:underline"
            onclick={() => (cvSelected = null)}
          >Change selection</button>
        </div>
        <label class="mb-3 block">
          <span class="mb-1 block text-xs font-medium text-slate-600">
            Issue number (optional — leave blank to use the filename / ComicInfo)
          </span>
          <input
            type="text"
            class="w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            placeholder="e.g. 12, Annual 1, ½"
            bind:value={cvIssueNumberInput}
          />
        </label>
        <div class="flex justify-end gap-2">
          <Button variant="ghost" onclick={closeChange}>Cancel</Button>
          <Button onclick={submitCvMatch} loading={cvSubmitting}>Match</Button>
        </div>
      {:else}
        <CvSearchInput
          initialQuery={initialHint}
          onSelect={(r) => {
            cvSelected = r;
          }}
        />
        <p class="mt-3 text-xs text-slate-500">
          Adding a series from search will also queue a series-wide rematch for sibling files in
          your library.
        </p>
      {/if}
    {:else}
      <p class="mb-2 text-sm">
        Enter the issue id (numeric DB id, not the issue number) to assign. Find one from
        <a class="text-blue-600 hover:underline" href="/series">the series detail page</a>.
      </p>
      <input
        type="number"
        class="w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        placeholder="Issue id"
        bind:value={issueIdInput}
      />
      <div class="mt-4 flex justify-end gap-2">
        <Button variant="ghost" onclick={closeChange}>Cancel</Button>
        <Button onclick={submitIssueId}>Save</Button>
      </div>
    {/if}
  {/if}
</Modal>

<Modal open={folderModalFolder !== null} title="Match folder to ComicVine" onClose={closeFolderMatch}>
  {#if folderModalFolder !== null}
    <p class="mb-1 text-xs text-slate-500">Folder</p>
    <p class="mb-3 truncate font-mono text-sm" title={folderModalFolder}>
      {folderModalFolder || '(library root)'}
    </p>
    <p class="mb-4 text-xs text-slate-500">
      {folderActionableCount} actionable file{folderActionableCount === 1 ? '' : 's'} —
      already-owned and ignored files are skipped automatically.
    </p>

    {#if folderCvSelected}
      <div class="mb-3 rounded-md border border-slate-200 bg-slate-50 p-3 text-sm">
        <div class="font-medium">
          {folderCvSelected.name}{#if folderCvSelected.start_year} <span class="text-slate-500">({folderCvSelected.start_year})</span>{/if}
        </div>
        <div class="text-xs text-slate-500">
          {folderCvSelected.publisher ?? 'Unknown publisher'} ·
          {folderCvSelected.issue_count} issue{folderCvSelected.issue_count === 1 ? '' : 's'}
        </div>
        <button
          type="button"
          class="mt-1 text-xs text-blue-700 hover:underline"
          onclick={() => (folderCvSelected = null)}
        >Change selection</button>
      </div>
      <div class="flex justify-end gap-2">
        <Button variant="ghost" onclick={closeFolderMatch}>Cancel</Button>
        <Button onclick={submitFolderMatch} loading={folderSubmitting}>
          Match {folderActionableCount} file{folderActionableCount === 1 ? '' : 's'}
        </Button>
      </div>
    {:else}
      <CvSearchInput
        initialQuery={folderHint}
        onSelect={(r) => {
          folderCvSelected = r;
        }}
      />
      <p class="mt-3 text-xs text-slate-500">
        Adding a series will also queue a series-wide rematch — sibling files outside this folder
        may pick up the match too.
      </p>
    {/if}
  {/if}
</Modal>
