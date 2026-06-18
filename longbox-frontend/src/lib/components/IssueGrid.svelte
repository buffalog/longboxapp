<script lang="ts">
  import { onMount } from 'svelte';
  import type { IssueWithFile } from '$lib/types';
  import IssueRow from './IssueRow.svelte';

  interface Props {
    issues: IssueWithFile[];
    /** Parent series id — passed through to each IssueRow so the
     *  per-issue "Search" button on Missing rows can construct its
     *  endpoint URL. Optional; omitted at callers that don't carry a
     *  single parent series (the button hides when absent). */
    seriesId?: number;
    /** Forwarded to each IssueRow — opens the Interactive Search modal
     *  for a Missing issue. */
    onInteractiveSearch?: (issue: IssueWithFile) => void;
  }

  let { issues, seriesId, onInteractiveSearch }: Props = $props();

  let gridRef: HTMLElement | null = $state(null);

  // j/k navigation across issue title buttons. Brief: matches /files
  // triage pattern; no a/m/i shortcuts here (series detail is read-only
  // catalog data). Focused index is implicit — we read it from the
  // currently-active element's row position rather than tracking state.
  // Suppressed inside form controls so typing in the page-level filter
  // (if any) isn't hijacked. Suppressed when a modifier key is held
  // so browser shortcuts (Cmd+J, etc.) still work.
  function isTypingTarget(el: EventTarget | null): boolean {
    if (!(el instanceof HTMLElement)) return false;
    const tag = el.tagName.toLowerCase();
    return tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable;
  }

  function moveFocus(delta: 1 | -1): void {
    if (!gridRef) return;
    const buttons = Array.from(
      gridRef.querySelectorAll<HTMLElement>('[data-issue-title]')
    );
    if (buttons.length === 0) return;
    const current = document.activeElement;
    let idx = buttons.findIndex((b) => b === current);
    if (idx === -1) {
      // No row focused — j starts at top, k starts at bottom.
      idx = delta === 1 ? -1 : buttons.length;
    }
    const next = Math.max(0, Math.min(buttons.length - 1, idx + delta));
    buttons[next]?.focus();
  }

  function onGlobalKey(e: KeyboardEvent): void {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (isTypingTarget(e.target)) return;
    if (e.key === 'j') {
      e.preventDefault();
      moveFocus(1);
    } else if (e.key === 'k') {
      e.preventDefault();
      moveFocus(-1);
    }
  }

  onMount(() => {
    window.addEventListener('keydown', onGlobalKey);
    return () => window.removeEventListener('keydown', onGlobalKey);
  });
</script>

<div bind:this={gridRef} class="overflow-hidden rounded-lg border border-slate-200 bg-white">
  <table class="w-full text-left">
    <thead class="bg-slate-50 text-xs uppercase tracking-wide text-slate-500">
      <tr>
        <th class="px-3 py-2 w-14">Cover</th>
        <th class="px-3 py-2">#</th>
        <th class="px-3 py-2">ID</th>
        <th class="px-3 py-2">Title / File</th>
        <th class="px-3 py-2">Cover date</th>
        <th class="px-3 py-2">Status</th>
      </tr>
    </thead>
    <tbody>
      {#each issues as issue (issue.id)}
        <IssueRow {issue} {seriesId} {onInteractiveSearch} />
      {/each}
    </tbody>
  </table>
</div>
