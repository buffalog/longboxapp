<script lang="ts">
  import { goto, invalidate } from '$app/navigation';
  import { Files } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    clearFileIgnored,
    markFileIgnored,
    matchFileFromCv,
    setFileIssue
  } from '$lib/api/files';
  import Button from '$lib/components/Button.svelte';
  import CvSearchInput from '$lib/components/CvSearchInput.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import FileRow from '$lib/components/FileRow.svelte';
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

  const statusOptions: Array<{ value: FileStatus | 'all'; label: string }> = [
    { value: 'needs_review', label: 'Needs review' },
    { value: 'unmatched', label: 'Unmatched' },
    { value: 'ignored', label: 'Ignored' },
    { value: 'owned', label: 'Owned' },
    { value: 'all', label: 'All' }
  ];

  const initialHint = $derived(
    pendingChange ? searchHintFromPath(pendingChange.path_relative) : ''
  );

  function setStatus(s: FileStatus | 'all'): void {
    const url = new URL(window.location.href);
    url.searchParams.set('status', s);
    void goto(url.pathname + url.search, { replaceState: true });
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
</script>

<h1 class="mb-4 text-2xl font-bold">Files</h1>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

<div class="mb-4 flex flex-wrap gap-2" role="tablist" aria-label="File status filter">
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

{#if data.files.length === 0}
  <EmptyState
    icon={Files}
    title={data.status === 'needs_review' ? 'Nothing needs review' : 'No files in this view'}
    message="Run a scan to populate the catalog, or pick a different filter."
  />
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
