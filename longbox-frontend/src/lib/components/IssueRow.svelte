<script lang="ts">
  import {
    CheckCircle2,
    Circle,
    Clock,
    AlertCircle,
    XCircle,
    ExternalLink,
    FileImage,
    Search
  } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { searchIssueNow } from '$lib/api/pull';
  import type { IssueWithFile } from '$lib/types';
  import { cvIssueUrl, formatDate } from '$lib/format';
  import { sanitizeCvSynopsis } from '$lib/text';
  import { isSolicited } from '$lib/solicitation';
  import { toast } from '$lib/stores/toast.svelte';

  interface Props {
    issue: IssueWithFile;
    /** Parent series id — needed to construct the per-issue search
     *  endpoint URL. Optional because IssueRow is also rendered in
     *  contexts that don't have a single parent series (e.g. the
     *  needs-attention listing); when absent, the Search button is
     *  hidden. */
    seriesId?: number;
  }

  let { issue, seriesId }: Props = $props();

  // Per-row Search debounce — 15 s, same trade-off as the per-series
  // Search-now button on the pull list page. The backend's in-flight
  // guard catches duplicate fires silently, but a debounce here keeps
  // rapid clicks from spamming the API with no-op work.
  const SEARCH_BUTTON_DISABLED_MS = 15_000;
  let searching = $state(false);

  // A file-present row keeps its actionable status. A no-file row
  // splits into 'solicited' (cover_date today-or-future — not shipped
  // yet) vs plain 'missing'.
  const status = $derived.by(() => {
    if (!issue.file) {
      return isSolicited(issue.cover_date) ? ('solicited' as const) : ('missing' as const);
    }
    return issue.file.status;
  });

  const statusMeta = $derived.by(() => {
    switch (status) {
      case 'owned':
        return { Icon: CheckCircle2, color: 'text-status-owned', label: 'Owned' };
      case 'needs_review':
        return { Icon: AlertCircle, color: 'text-status-needs_review', label: 'Needs review' };
      case 'ignored':
        return { Icon: XCircle, color: 'text-status-ignored', label: 'Ignored' };
      case 'unmatched':
        return { Icon: AlertCircle, color: 'text-status-unmatched', label: 'Unmatched' };
      case 'solicited':
        return { Icon: Clock, color: 'text-status-solicited', label: 'Solicited' };
      case 'missing':
        return { Icon: Circle, color: 'text-status-missing', label: 'Missing' };
    }
  });

  // Whether to surface the Search button for this row. Only Missing
  // issues qualify — Owned doesn't need it, Solicited isn't shipped,
  // Needs-review / Ignored / Unmatched belong to other workflows.
  // seriesId is required to construct the API URL; without it, hide.
  const showSearchButton = $derived(seriesId !== undefined && status === 'missing');

  async function handleSearchNow(): Promise<void> {
    if (searching || seriesId === undefined) return;
    searching = true;
    setTimeout(() => {
      searching = false;
    }, SEARCH_BUTTON_DISABLED_MS);
    try {
      await searchIssueNow(seriesId, issue.id);
      toast.success(`Search started for #${issue.number}.`);
    } catch (e) {
      // The backend's in-flight guard silently no-ops rather than
      // returning a 409, so the only errors expected here are real
      // (404 for a deleted issue, network failure). Surface them as
      // warnings rather than ignoring.
      const message =
        e instanceof ApiError ? e.message : 'Could not start the search.';
      toast.warning(message);
    }
  }

  // Click-to-copy feedback goes through the shared toast store
  // (Task 5). The button label stays the issue id permanently;
  // success / failure surface as corner toasts.
  async function copyId(): Promise<void> {
    const text = String(issue.id);
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard API unavailable');
      await navigator.clipboard.writeText(text);
      toast.success(`Issue ID ${text} copied`);
    } catch {
      // Plain-HTTP over LAN or older browsers — `navigator.clipboard`
      // may not exist in non-secure contexts.
      toast.error('Copy failed — clipboard needs HTTPS or a modern browser');
    }
  }

  // --- Row expand state (Task 1) ---
  //
  // `expanded` toggles the inline detail section below the row. The
  // title cell is the click target (brief: not whole-row, not chevron)
  // so other interactive cells (Copy ID, status pill) keep their own
  // click semantics. Esc on the focused title button collapses.
  //
  // `cachedSanitizedSynopsis` is the lazy-sanitization seam: DOMPurify
  // runs on first expand and the result sticks for subsequent
  // expand/collapse cycles on the same row.
  let expanded = $state(false);
  let cachedSanitizedSynopsis = $state<string | null>(null);

  function toggleExpand(): void {
    expanded = !expanded;
    if (expanded && cachedSanitizedSynopsis === null && issue.summary) {
      cachedSanitizedSynopsis = sanitizeCvSynopsis(issue.summary);
    }
  }

  function onTitleKey(e: KeyboardEvent): void {
    if (e.key === 'Escape' && expanded) {
      e.preventDefault();
      expanded = false;
    }
  }

  const cvUrl = $derived(issue.cv_issue_id ? cvIssueUrl(issue.cv_issue_id) : null);
</script>

<tr class="border-b border-slate-100 last:border-b-0">
  <td class="px-3 py-2 align-top">
    <div class="size-12 flex-shrink-0 overflow-hidden rounded bg-slate-100">
      {#if issue.cover_url}
        <img src={issue.cover_url} alt="" class="size-full object-cover" loading="lazy" />
      {:else}
        <div class="flex size-full items-center justify-center text-slate-400">
          <FileImage class="size-5" aria-hidden="true" />
        </div>
      {/if}
    </div>
  </td>
  <td class="px-3 py-2 align-top font-mono text-sm tabular-nums text-slate-700">
    #{issue.number}
  </td>
  <td class="px-3 py-2 align-top">
    <button
      type="button"
      onclick={copyId}
      title="Copy issue id to clipboard"
      aria-label={`Copy issue id ${issue.id}`}
      class="inline-flex rounded font-mono text-xs tabular-nums text-slate-500 hover:text-slate-800 hover:underline focus:outline-none focus:ring-2 focus:ring-blue-500"
    >{issue.id}</button>
  </td>
  <td class="px-3 py-2 align-top">
    <button
      type="button"
      data-issue-title
      onclick={toggleExpand}
      onkeydown={onTitleKey}
      aria-expanded={expanded}
      class="rounded text-left text-sm font-medium text-slate-900 hover:text-blue-700 focus:outline-none focus:ring-2 focus:ring-blue-500"
    >{issue.title ?? '—'}</button>
    {#if issue.file?.path_relative}
      <div class="font-mono text-xs text-slate-500">{issue.file.path_relative}</div>
    {/if}
  </td>
  <td class="px-3 py-2 align-top text-xs text-slate-500">{formatDate(issue.cover_date)}</td>
  <td class="px-3 py-2 align-top">
    <div class="flex items-center gap-2">
      <span class="inline-flex items-center gap-1 {statusMeta.color}" aria-label={statusMeta.label}>
        <statusMeta.Icon class="size-4" aria-hidden="true" />
        <span class="text-xs font-medium">{statusMeta.label}</span>
      </span>
      {#if showSearchButton}
        <button
          type="button"
          onclick={handleSearchNow}
          disabled={searching}
          aria-label={`Search for issue ${issue.number}`}
          class="inline-flex items-center gap-1 rounded border border-slate-200 px-1.5 py-0.5 text-xs font-medium text-slate-600 hover:bg-slate-50 hover:text-slate-900 focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
        >
          <Search class="size-3" aria-hidden="true" />
          Search
        </button>
      {/if}
    </div>
  </td>
</tr>

<tr aria-hidden={!expanded}>
  <td colspan="6" class="p-0">
    <!-- Accordion via max-height transition (brief: 200-300ms). 600px
         is a generous cap for typical CV synopses; longer ones still
         render but scroll naturally within the row in practice. -->
    <div
      class="overflow-hidden transition-[max-height] duration-300 ease-in-out"
      style:max-height={expanded ? '600px' : '0px'}
    >
      <div class="border-t border-slate-100 bg-slate-50 px-3 py-3">
        <div class="flex gap-4">
          <!-- Larger cover (~80x120). 2:3 aspect is standard for comic
               covers; w-20 h-30 → exact 80×120 isn't a Tailwind stock
               size, w-20 h-[7.5rem] gets us 80×120 at base font. -->
          <div class="h-[7.5rem] w-20 flex-shrink-0 overflow-hidden rounded bg-slate-100">
            {#if issue.cover_url}
              <img src={issue.cover_url} alt="" class="size-full object-cover" loading="lazy" />
            {:else}
              <div class="flex size-full items-center justify-center text-slate-400">
                <FileImage class="size-8" aria-hidden="true" />
              </div>
            {/if}
          </div>
          <div class="min-w-0 flex-1">
            {#if cachedSanitizedSynopsis}
              <div class="prose prose-sm max-w-none text-sm text-slate-700">
                {@html cachedSanitizedSynopsis}
              </div>
            {:else}
              <p class="text-sm italic text-slate-500">No synopsis available.</p>
            {/if}
            {#if cvUrl}
              <div class="mt-3">
                <a
                  href={cvUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  class="inline-flex items-center gap-1 text-sm font-medium text-blue-600 hover:underline"
                >View on ComicVine <ExternalLink class="size-3.5" aria-hidden="true" /></a>
              </div>
            {/if}
          </div>
        </div>
      </div>
    </div>
  </td>
</tr>
