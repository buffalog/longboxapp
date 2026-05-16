<script lang="ts">
  import { goto, invalidate } from '$app/navigation';
  import { Files } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    clearFileIgnored,
    markFileIgnored,
    setFileIssue
  } from '$lib/api/files';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import FileRow from '$lib/components/FileRow.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import type { EnrichedFileRow, FileStatus } from '$lib/types';

  let { data } = $props();

  let error = $state<ApiError | null>(null);
  let busyId = $state<number | null>(null);
  let pendingChange = $state<EnrichedFileRow | null>(null);
  let changeInput = $state('');

  const statusOptions: Array<{ value: FileStatus | 'all'; label: string }> = [
    { value: 'needs_review', label: 'Needs review' },
    { value: 'unmatched', label: 'Unmatched' },
    { value: 'ignored', label: 'Ignored' },
    { value: 'owned', label: 'Owned' },
    { value: 'all', label: 'All' }
  ];

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
    changeInput = file.issue?.id ? String(file.issue.id) : '';
  }

  function closeChange(): void {
    pendingChange = null;
    changeInput = '';
  }

  async function submitChange(): Promise<void> {
    if (!pendingChange) return;
    const issueId = Number(changeInput);
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
  <p class="mb-2 text-sm">
    Enter the issue id to assign. Find an issue id from
    <a class="text-blue-600 hover:underline" href="/series">the series detail page</a> (each
    issue row's URL contains it).
  </p>
  <input
    type="number"
    class="w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
    placeholder="Issue id"
    bind:value={changeInput}
  />
  <div class="mt-4 flex justify-end gap-2">
    <button
      type="button"
      class="rounded-md px-3 py-1.5 text-sm hover:bg-slate-100"
      onclick={closeChange}
    >Cancel</button>
    <button
      type="button"
      class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-800"
      onclick={submitChange}
    >Save</button>
  </div>
</Modal>
