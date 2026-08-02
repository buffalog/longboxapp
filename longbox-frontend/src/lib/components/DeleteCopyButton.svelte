<script lang="ts">
  // Delete ONE redundant copy, from ONE group, behind an explicit
  // confirmation that names the file and what it is filed under.
  //
  // There is deliberately no "delete all", no select-many, and no
  // suggested keeper. A bulk control is the shape that arms a row of
  // delete buttons the user never read individually, and a suggestion
  // right most of the time stops them reading the evidence at all —
  // which is the evidence this whole page exists to show.
  //
  // The two texts here answer different questions and come from
  // different places, which is the point:
  //
  //   BEFORE — what am I about to delete, and what is it filed under?
  //            Known from evidence already on screen.
  //   AFTER  — what actually happened to that issue?
  //            Read from the RESPONSE. Never inferred from the fact
  //            that the delete succeeded.
  //
  // The second rule is not theoretical. Across the 37 live groups the
  // deleted copy's issue reverts to missing 36 times and does not once:
  // `Hello Darkness (2025) 002.cbz` shares its issue with a file in the
  // 2024 folder that is not in the group at all. That case is invisible
  // from inside the group, so the server re-queries ownership after the
  // delete and this reads the answer.
  import { Trash2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    deleteDuplicate,
    describeRevert,
    type FileEvidence,
    type DeleteDuplicateResult
  } from '$lib/api/integrity';

  interface Props {
    digest: string;
    file: FileEvidence;
    /** Copies in this group, so the last one cannot be offered for deletion. */
    groupSize: number;
    /** Called after a successful delete so the page can re-read findings. */
    onDeleted: (result: DeleteDuplicateResult) => void;
  }
  let { digest, file, groupSize, onDeleted }: Props = $props();

  let confirming = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  // What it is filed under — stated as a fact, with no claim about what
  // deleting it will do to that issue. The server decides that and the
  // response says so.
  const boundTo = $derived(
    file.series_title
      ? `${file.series_title} #${file.issue_number ?? '?'}`
      : 'no issue (an unfiled stray)'
  );

  async function confirm(): Promise<void> {
    busy = true;
    error = null;
    try {
      const result = await deleteDuplicate(digest, file.file_id);
      confirming = false;
      onDeleted(result);
    } catch (e) {
      // The server refuses for reasons the user needs in full — an
      // alias pair, a stale digest, a group that would end up empty.
      // Show its sentence rather than a generic failure.
      error = e instanceof ApiError ? e.message : 'The delete failed.';
    } finally {
      busy = false;
    }
  }
</script>

{#if groupSize <= 1}
  <!-- Nothing to be redundant with. The server refuses this too; not
       offering it is the honest surface. -->
  <span class="text-xs text-slate-400">only copy</span>
{:else if !confirming}
  <button
    type="button"
    class="inline-flex items-center gap-1 rounded border border-slate-300 px-2 py-1 text-xs text-slate-700 hover:border-red-300 hover:bg-red-50 hover:text-red-800"
    onclick={() => {
      confirming = true;
      error = null;
    }}
  >
    <Trash2 class="size-3" aria-hidden="true" />
    Delete this copy
  </button>
{:else}
  <span class="inline-flex flex-col items-end gap-1">
    <span class="text-xs text-slate-700">
      Delete <span class="font-mono break-all">{file.path_relative}</span>?
    </span>
    <span class="text-xs text-slate-500">
      Filed under {boundTo}. If it is the only file that issue owns, the issue reverts to missing.
    </span>
    <span class="flex gap-1">
      <button
        type="button"
        class="rounded bg-red-600 px-2 py-1 text-xs font-medium text-white hover:bg-red-700 disabled:opacity-50"
        disabled={busy}
        onclick={confirm}
      >
        {busy ? 'Deleting…' : 'Delete it'}
      </button>
      <button
        type="button"
        class="rounded border border-slate-300 px-2 py-1 text-xs text-slate-700 hover:bg-slate-50 disabled:opacity-50"
        disabled={busy}
        onclick={() => {
          confirming = false;
          error = null;
        }}
      >
        Cancel
      </button>
    </span>
    {#if error}
      <span class="max-w-md text-right text-xs text-red-700">{error}</span>
    {/if}
  </span>
{/if}
