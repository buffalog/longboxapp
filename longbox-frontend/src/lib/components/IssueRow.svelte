<script lang="ts">
  import { CheckCircle2, Circle, AlertCircle, XCircle } from 'lucide-svelte';
  import type { IssueWithFile } from '$lib/types';
  import { formatDate } from '$lib/format';

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

  // Component-local click-to-copy feedback. We don't have a global toast
  // system; for one button on one table it's not worth standing one up.
  // The label swaps to "Copied!" (or "Copy failed") for ~1.5s then back.
  let copyState = $state<'idle' | 'copied' | 'failed'>('idle');
  let resetTimer: ReturnType<typeof setTimeout> | null = null;

  async function copyId(): Promise<void> {
    const text = String(issue.id);
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard API unavailable');
      await navigator.clipboard.writeText(text);
      copyState = 'copied';
    } catch {
      // Plain-HTTP over LAN or older browsers — `navigator.clipboard` may
      // not exist in non-secure contexts. Surface inline so the user can
      // still select-copy from the column.
      copyState = 'failed';
    }
    if (resetTimer !== null) clearTimeout(resetTimer);
    resetTimer = setTimeout(() => {
      copyState = 'idle';
    }, 1500);
  }

  const idLabel = $derived(
    copyState === 'copied' ? 'Copied!' : copyState === 'failed' ? 'Copy failed' : String(issue.id)
  );
</script>

<tr class="border-b border-slate-100 last:border-b-0">
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
      class:text-emerald-700={copyState === 'copied'}
      class:text-red-700={copyState === 'failed'}
    >{idLabel}</button>
  </td>
  <td class="px-3 py-2 align-top">
    <div class="text-sm font-medium text-slate-900">{issue.title ?? '—'}</div>
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
