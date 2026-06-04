<script lang="ts">
  // The needs-attention surface — two sections: pull-engine failures
  // (retryable here) and Phase B manual-intervention failures (resolved
  // on disk; the fs watcher re-triggers processing).
  import { CircleSlash } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    clearAllPullFailures,
    dismissPullFailure,
    retryPull,
    type PullFailure
  } from '$lib/api/needs_attention';
  import { toast } from '$lib/stores/toast.svelte';
  import { formatBytes, formatRelative } from '$lib/format';
  import Button from '$lib/components/Button.svelte';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import type { InterventionReason } from '$lib/types';

  let { data } = $props();

  // Pull failures are self-owned — retry / dismiss / clear-all all drop
  // rows from local state optimistically. Phase B's pending list is
  // read-only here (resolved on disk, not in the UI).
  let pullFailures = $state<PullFailure[]>([...data.pullFailures]);
  let error = $state<ApiError | null>(null);
  let retryingIssue = $state<number | null>(null);
  let dismissingAttempt = $state<number | null>(null);
  let clearingAll = $state(false);

  const pending = $derived(data.pending);

  function categoryLabel(category: string): string {
    switch (category) {
      case 'submission_failed':
        return 'Submission failed';
      case 'grab_failed':
        return 'Grab failed';
      default:
        return category;
    }
  }

  function reasonLabel(reason: InterventionReason): string {
    switch (reason.kind) {
      case 'conflict':
        return 'Conflict';
      case 'comic_info_write_failed':
        return 'ComicInfo write failed';
      case 'move_failed':
        return 'Move failed';
    }
  }

  function reasonDetail(reason: InterventionReason): string | null {
    return reason.kind === 'conflict' ? null : reason.detail;
  }

  async function handleRetry(failure: PullFailure): Promise<void> {
    retryingIssue = failure.issue_id;
    error = null;
    try {
      await retryPull(failure.series_id, failure.issue_id);
      pullFailures = pullFailures.filter((p) => p.issue_id !== failure.issue_id);
      toast.success(`Retrying ${failure.series_title} #${failure.issue_number}.`);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      retryingIssue = null;
    }
  }

  async function handleDismiss(failure: PullFailure): Promise<void> {
    dismissingAttempt = failure.id;
    error = null;
    try {
      await dismissPullFailure(failure.id);
      pullFailures = pullFailures.filter((p) => p.id !== failure.id);
      toast.success(`Dismissed ${failure.series_title} #${failure.issue_number}.`);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      dismissingAttempt = null;
    }
  }

  async function handleClearAll(): Promise<void> {
    if (clearingAll) return;
    clearingAll = true;
    error = null;
    const count = pullFailures.length;
    try {
      await clearAllPullFailures();
      pullFailures = [];
      toast.success(`Cleared ${count} pull failure${count === 1 ? '' : 's'}.`);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      clearingAll = false;
    }
  }
</script>

<header class="mb-4">
  <h1 class="text-2xl font-bold">Needs attention</h1>
  <p class="mt-1 text-sm text-slate-600">
    Pulls the engine couldn't complete, and files the post-processor couldn't place automatically.
  </p>
</header>

{#if error}
  <div class="mb-4"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
{/if}

{#if pullFailures.length === 0 && pending.count === 0}
  <EmptyState
    icon={CircleSlash}
    title="Nothing needs attention"
    message="No failed pulls and no stuck files. New failures appear here automatically."
  />
{:else}
  <!-- ===================== Pull failures ===================== -->
  <section class="mb-8">
    <header class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
      <h2 class="text-lg font-semibold">Pull failures</h2>
      {#if pullFailures.length > 0}
        <Button
          variant="ghost"
          size="sm"
          onclick={handleClearAll}
          loading={clearingAll}
          disabled={clearingAll}
        >
          Clear all
        </Button>
      {/if}
    </header>
    {#if pullFailures.length === 0}
      <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
        No failed pulls.
      </p>
    {:else}
      <div class="overflow-x-auto rounded-lg border border-slate-200 bg-white">
        <table class="w-full text-sm">
          <thead class="bg-slate-50 text-left text-xs uppercase text-slate-500">
            <tr>
              <th class="px-3 py-2">Series</th>
              <th class="px-3 py-2">Issue</th>
              <th class="px-3 py-2">Failure</th>
              <th class="px-3 py-2">Last attempt</th>
              <th class="px-3 py-2"></th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-100">
            {#each pullFailures as f (f.issue_id)}
              <tr>
                <td class="px-3 py-2">
                  <a
                    href={`/series/${f.series_id}`}
                    class="font-medium text-blue-600 hover:underline"
                  >
                    {f.series_title}
                  </a>
                </td>
                <td class="whitespace-nowrap px-3 py-2 font-mono text-slate-600">
                  #{f.issue_number}
                </td>
                <td class="px-3 py-2">
                  <span class="font-medium">{categoryLabel(f.category)}</span>
                  <span class="ml-1 text-xs text-slate-400">({f.retry_count} attempts)</span>
                  {#if f.error_message}
                    <div class="mt-0.5 text-xs text-slate-500">{f.error_message}</div>
                  {/if}
                </td>
                <td class="whitespace-nowrap px-3 py-2 text-xs text-slate-500">
                  {formatRelative(f.attempted_at)}
                </td>
                <td class="px-3 py-2 text-right">
                  <div class="inline-flex gap-2">
                    <Button
                      size="sm"
                      onclick={() => handleRetry(f)}
                      loading={retryingIssue === f.issue_id}
                      disabled={retryingIssue !== null || dismissingAttempt !== null || clearingAll}
                    >
                      Retry
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onclick={() => handleDismiss(f)}
                      loading={dismissingAttempt === f.id}
                      disabled={retryingIssue !== null || dismissingAttempt !== null || clearingAll}
                    >
                      Dismiss
                    </Button>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>

  <!-- ================= Manual intervention ================= -->
  <section>
    <h2 class="mb-1 text-lg font-semibold">Manual intervention</h2>
    <p class="mb-2 text-xs text-slate-500">
      Files the post-processor couldn't place. Resolve each on disk (move, rename, delete the
      conflicting target); processing re-triggers on the next filesystem event for that file.
    </p>
    {#if pending.count === 0}
      <p class="rounded-lg border border-slate-200 bg-white p-6 text-sm text-slate-500">
        No files are stuck.
      </p>
    {:else}
      <div class="overflow-x-auto rounded-lg border border-slate-200 bg-white">
        <table class="w-full text-sm">
          <thead class="bg-slate-50 text-left text-xs uppercase text-slate-500">
            <tr>
              <th class="px-3 py-2">Source</th>
              <th class="px-3 py-2">Target</th>
              <th class="px-3 py-2">Reason</th>
              <th class="px-3 py-2 text-right">Size</th>
              <th class="px-3 py-2">Last attempt</th>
            </tr>
          </thead>
          <tbody>
            {#each pending.items as item (item.source_path)}
              <tr class="border-t border-slate-100">
                <td class="break-all px-3 py-2 font-mono text-xs">{item.source_path}</td>
                <td class="break-all px-3 py-2 font-mono text-xs text-slate-600">
                  {item.target_path}
                </td>
                <td class="px-3 py-2">
                  <span class="font-medium">{reasonLabel(item.reason)}</span>
                  {#if reasonDetail(item.reason)}
                    <div class="mt-0.5 text-xs text-slate-500">{reasonDetail(item.reason)}</div>
                  {/if}
                </td>
                <td class="whitespace-nowrap px-3 py-2 text-right">{formatBytes(item.size)}</td>
                <td class="whitespace-nowrap px-3 py-2 text-xs text-slate-500">
                  {formatRelative(item.last_attempt)}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>
{/if}
