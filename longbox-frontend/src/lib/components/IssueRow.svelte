<script lang="ts">
  import { CheckCircle2, Circle, AlertCircle, XCircle, ExternalLink, FileImage } from 'lucide-svelte';
  import type { IssueWithFile } from '$lib/types';
  import { cvIssueUrl, formatDate } from '$lib/format';
  import { sanitizeCvSynopsis } from '$lib/text';
  import { toast } from '$lib/stores/toast.svelte';

  interface Props {
    issue: IssueWithFile;
  }

  let { issue }: Props = $props();

  const status = $derived.by(() => {
    if (!issue.file) return 'missing' as const;
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
      case 'missing':
        return { Icon: Circle, color: 'text-status-missing', label: 'Missing' };
    }
  });

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
    <span class="inline-flex items-center gap-1 {statusMeta.color}" aria-label={statusMeta.label}>
      <statusMeta.Icon class="size-4" aria-hidden="true" />
      <span class="text-xs font-medium">{statusMeta.label}</span>
    </span>
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
