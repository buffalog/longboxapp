<script lang="ts">
  import { AlertTriangle, Copy } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    correctDuplicateFile,
    listDuplicateFiles,
    resolveDuplicateFiles,
    type DupCandidate,
    type DupGroup,
    type Resolution
  } from '$lib/api/duplicateFiles';
  import { formatBytes } from '$lib/format';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import Modal from '$lib/components/Modal.svelte';

  const PER_PAGE = 50;

  let groups = $state<DupGroup[]>([]);
  let total = $state(0);
  let page = $state(1);
  let loading = $state(true);
  let error = $state<ApiError | null>(null);

  // issue_id -> chosen keep file_id. Seeded from each group's server
  // suggestion; the user can override before confirming.
  let keep = $state<Record<number, number>>({});

  // Confirmation is required before any delete. `pending` holds the groups a
  // confirmed resolve will act on (one group, or all duplicates on the page).
  let pending = $state<DupGroup[] | null>(null);
  let resolving = $state(false);

  // file_id -> issue the user will re-point that file to. Seeded from the
  // server's suggestion; overridable per file. 0 = "leave it alone".
  let moveTo = $state<Record<number, number>>({});
  // file_id currently being corrected, so only its own button spins.
  let correcting = $state<number | null>(null);

  const pageCount = $derived(Math.max(1, Math.ceil(total / PER_PAGE)));
  const duplicateGroups = $derived(groups.filter((g) => g.kind === 'duplicate'));

  $effect(() => {
    void loadPage(page);
  });

  async function loadPage(p: number): Promise<void> {
    loading = true;
    error = null;
    try {
      const res = await listDuplicateFiles(p, PER_PAGE);
      groups = res.groups;
      total = res.total;
      // Seed keep-selection from the server suggestion for each dup group.
      const next: Record<number, number> = {};
      const nextMove: Record<number, number> = {};
      for (const g of res.groups) {
        if (g.kind === 'duplicate') {
          const fid = g.suggested_keep_file_id ?? g.files[0]?.file_id;
          if (fid !== undefined) next[g.issue_id] = fid;
        } else {
          for (const f of g.files) nextMove[f.file_id] = f.suggested_issue_id ?? 0;
        }
      }
      keep = next;
      moveTo = nextMove;
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
      groups = [];
    } finally {
      loading = false;
    }
  }

  function keepFor(g: DupGroup): number {
    // -1 is unreachable in practice (dup groups always have files); it would
    // fail the server's "keep must be a present candidate" guard rather than
    // delete anything, which is the safe degenerate outcome.
    return keep[g.issue_id] ?? g.suggested_keep_file_id ?? g.files[0]?.file_id ?? -1;
  }

  function numberOf(g: DupGroup, issueId: number): string {
    return g.issue_options.find((o) => o.issue_id === issueId)?.number ?? '?';
  }

  // Re-point one file. Not destructive — no confirm modal, the button click
  // IS the confirmation. Reload after: the group may reclassify (or vanish)
  // once the stray leaves, and the server is the authority on that.
  async function correctFile(g: DupGroup, f: DupCandidate): Promise<void> {
    const target = moveTo[f.file_id];
    if (!target) return;
    correcting = f.file_id;
    try {
      await correctDuplicateFile(f.file_id, target);
      toast.success(`Moved to ${g.series_title} #${numberOf(g, target)}.`);
      await loadPage(page);
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : String(e));
    } finally {
      correcting = null;
    }
  }

  function deleteCountFor(gs: DupGroup[]): number {
    return gs.reduce((n, g) => n + (g.files.length - 1), 0);
  }

  function requestResolve(gs: DupGroup[]): void {
    if (gs.length > 0) pending = gs;
  }

  async function confirmResolve(): Promise<void> {
    if (!pending) return;
    const target = pending;
    resolving = true;
    try {
      const resolutions: Resolution[] = target.map((g) => ({
        issue_id: g.issue_id,
        keep_file_id: keepFor(g)
      }));
      const { results } = await resolveDuplicateFiles(resolutions);

      const resolvedIds = new Set<number>();
      let deleted = 0;
      let refused = 0;
      let failed = 0;
      for (const r of results) {
        if (r.status === 'resolved') {
          resolvedIds.add(r.issue_id);
          deleted += r.deleted_file_ids.length;
          failed += r.failed.length;
        } else {
          refused += 1;
        }
      }
      // Drop resolved groups from the view; leave refused ones visible.
      groups = groups.filter((g) => !resolvedIds.has(g.issue_id));
      total = Math.max(0, total - resolvedIds.size);

      let msg = `Deleted ${deleted} duplicate file${deleted === 1 ? '' : 's'}.`;
      if (refused > 0) msg += ` ${refused} group${refused === 1 ? '' : 's'} refused (see list).`;
      if (failed > 0) msg += ` ${failed} file${failed === 1 ? '' : 's'} could not be deleted.`;
      if (failed > 0 || refused > 0) toast.warning(msg);
      else toast.success(msg);

      // If the page emptied but more remain, pull the (now shifted) page.
      if (groups.length === 0 && total > 0) {
        page = Math.min(page, Math.max(1, Math.ceil(total / PER_PAGE)));
        await loadPage(page);
      }
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : String(e));
    } finally {
      resolving = false;
      pending = null;
    }
  }
</script>

<section class="rounded-lg border border-slate-200 bg-white p-4">
  <header class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
    <h2 class="flex items-center gap-2 text-base font-semibold">
      <Copy class="size-4 text-slate-500" aria-hidden="true" />
      Duplicate files
      {#if total > 0}<span class="text-sm font-normal text-slate-500">({total})</span>{/if}
    </h2>
    {#if duplicateGroups.length > 0}
      <Button
        size="sm"
        variant="ghost"
        disabled={loading || resolving}
        onclick={() => requestResolve(duplicateGroups)}
      >
        Resolve all on this page ({duplicateGroups.length})
      </Button>
    {/if}
  </header>

  <p class="mb-3 text-sm text-slate-600">
    Issues with more than one physical file on disk. Pick the copy to keep; the others are
    permanently deleted from disk. Groups flagged “not a duplicate” hold distinct issues wrongly
    sharing one record — those are fixed by moving each file to its real issue, not by deleting
    anything.
  </p>

  {#if error}
    <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
  {/if}

  {#if loading}
    <p class="text-sm text-slate-500">Loading…</p>
  {:else if groups.length === 0}
    <p class="text-sm text-slate-500">No duplicate files. 🎉</p>
  {:else}
    <ul class="space-y-3">
      {#each groups as g (g.issue_id)}
        <li class="rounded-md border border-slate-200 p-3">
          <div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
            <div class="text-sm font-medium">
              {g.series_title}
              <span class="text-slate-500">#{g.issue_number}</span>
            </div>
            {#if g.kind === 'mismatch'}
              <span
                class="inline-flex items-center gap-1 rounded bg-amber-50 px-1.5 py-0.5 text-xs font-medium text-amber-800"
              >
                <AlertTriangle class="size-3" aria-hidden="true" /> Not a duplicate — needs review
              </span>
            {/if}
          </div>

          {#if g.kind === 'mismatch'}
            <p class="mb-2 text-xs text-amber-800">
              These files parse to different issue numbers, so they’re distinct issues wrongly
              matched to one record — usually because a file’s embedded ComicInfo.xml carries the
              wrong issue number. Nothing here is deleted: move each stray file to the issue its
              filename says it is. Files with no suggestion need a manual call.
            </p>
          {/if}

          <ul class="divide-y divide-slate-100 rounded border border-slate-100">
            {#each g.files as f (f.file_id)}
              <li class="flex items-start gap-3 px-3 py-2 text-sm">
                {#if g.kind === 'duplicate'}
                  <input
                    type="radio"
                    class="mt-1 border-slate-300"
                    name={`keep-${g.issue_id}`}
                    value={f.file_id}
                    checked={keepFor(g) === f.file_id}
                    onchange={() => (keep = { ...keep, [g.issue_id]: f.file_id })}
                    disabled={resolving}
                    aria-label={`Keep ${f.path_relative}`}
                  />
                {/if}
                <div class="min-w-0 flex-1">
                  <div class="break-all">{f.path_relative}</div>
                  <div class="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-slate-500">
                    <span>{formatBytes(f.size_bytes)}</span>
                    <span class="uppercase">{f.format}</span>
                    {#if f.parsed_number}<span>#{f.parsed_number}</span>{/if}
                    {#if f.is_served}
                      <span class="rounded bg-blue-50 px-1 font-medium text-blue-700">served</span>
                    {/if}
                    {#if f.under_unsorted}
                      <span class="rounded bg-slate-100 px-1">_unsorted</span>
                    {/if}
                    {#if f.suspect_corrupt}
                      <span
                        class="inline-flex items-center gap-1 rounded bg-red-50 px-1 font-medium text-red-700"
                      >
                        <AlertTriangle class="size-3" aria-hidden="true" /> possibly corrupt
                      </span>
                    {/if}
                  </div>

                  {#if g.kind === 'mismatch'}
                    <div class="mt-1.5 flex flex-wrap items-center gap-2 text-xs">
                      <label class="text-slate-500" for={`move-${f.file_id}`}>Move to</label>
                      <select
                        id={`move-${f.file_id}`}
                        class="rounded border border-slate-300 px-1.5 py-1 text-xs"
                        value={moveTo[f.file_id] ?? 0}
                        onchange={(e) =>
                          (moveTo = {
                            ...moveTo,
                            [f.file_id]: Number(e.currentTarget.value)
                          })}
                        disabled={correcting !== null}
                      >
                        <option value={0}>Leave on #{g.issue_number}</option>
                        {#each g.issue_options as o (o.issue_id)}
                          {#if o.issue_id !== g.issue_id}
                            <option value={o.issue_id}>#{o.number}</option>
                          {/if}
                        {/each}
                      </select>
                      {#if f.suggested_issue_id}
                        <span class="text-slate-500">
                          suggested: #{numberOf(g, f.suggested_issue_id)}
                        </span>
                      {:else}
                        <span class="text-amber-800">no confident suggestion — your call</span>
                      {/if}
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={!moveTo[f.file_id] || correcting !== null}
                        loading={correcting === f.file_id}
                        onclick={() => correctFile(g, f)}
                      >
                        Move
                      </Button>
                    </div>
                  {/if}
                </div>
              </li>
            {/each}
          </ul>

          {#if g.kind === 'duplicate'}
            <div class="mt-2 flex justify-end">
              <Button
                size="sm"
                disabled={resolving}
                onclick={() => requestResolve([g])}
              >
                Delete {g.files.length - 1} other{g.files.length - 1 === 1 ? '' : 's'}
              </Button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>

    {#if pageCount > 1}
      <div class="mt-3 flex items-center justify-between text-sm">
        <Button size="sm" variant="ghost" disabled={page <= 1 || loading} onclick={() => (page -= 1)}
          >← Prev</Button
        >
        <span class="text-slate-500">Page {page} of {pageCount}</span>
        <Button
          size="sm"
          variant="ghost"
          disabled={page >= pageCount || loading}
          onclick={() => (page += 1)}>Next →</Button
        >
      </div>
    {/if}
  {/if}
</section>

{#if pending}
  <Modal open={true} title="Delete duplicate files?" onClose={() => (pending = null)}>
    <p class="text-sm text-slate-700">
      This permanently deletes <strong>{deleteCountFor(pending)}</strong> file{deleteCountFor(
        pending
      ) === 1
        ? ''
        : 's'} from disk
      {#if pending.length === 1}
        for <strong>{pending[0]?.series_title} #{pending[0]?.issue_number}</strong>, keeping the
        copy you selected.
      {:else}
        across <strong>{pending.length}</strong> issues, keeping the selected copy in each.
      {/if}
      This cannot be undone.
    </p>
    <div class="mt-4 flex justify-end gap-2">
      <Button variant="ghost" onclick={() => (pending = null)} disabled={resolving}>Cancel</Button>
      <Button variant="danger" loading={resolving} onclick={confirmResolve}>Delete files</Button>
    </div>
  </Modal>
{/if}
