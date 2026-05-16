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
</script>

<tr class="border-b border-slate-100 last:border-b-0">
  <td class="px-3 py-2 align-top font-mono text-sm tabular-nums text-slate-700">
    #{issue.number}
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
