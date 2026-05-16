<script lang="ts">
  import type { IssueWithFile } from '$lib/types';
  import IssueRow from './IssueRow.svelte';

  interface Props {
    issues: IssueWithFile[];
  }

  let { issues }: Props = $props();
</script>

<div class="overflow-hidden rounded-lg border border-slate-200 bg-white">
  <table class="w-full text-left">
    <thead class="bg-slate-50 text-xs uppercase tracking-wide text-slate-500">
      <tr>
        <th class="px-3 py-2">#</th>
        <th class="px-3 py-2">Title / File</th>
        <th class="px-3 py-2">Cover date</th>
        <th class="px-3 py-2">Status</th>
      </tr>
    </thead>
    <tbody>
      {#each issues as issue (issue.id)}
        <IssueRow {issue} />
      {/each}
    </tbody>
  </table>
</div>
