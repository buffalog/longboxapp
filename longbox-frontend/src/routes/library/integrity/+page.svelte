<script lang="ts">
  // Library Integrity — discovery.
  //
  // This VIEW makes no destructive request: the only thing it can write
  // is "Analyze content", whose surface is the digest and archive-label
  // columns. But the server now also exposes a per-group delete
  // (`POST /library/integrity/duplicates/delete`) that this page does
  // not yet call, so do not describe the FEATURE as read-only — only
  // this page's current request set. The distinction matters: a page
  // claiming a safety property the server no longer has is the same
  // defect as a module doc claiming it, and both were true here at one
  // point.
  import { FileSearch, RefreshCw } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    describeRevert,
    getAnalyzeStatus,
    getFindings,
    startAnalyze,
    walkIsConclusive,
    type AnalyzeStatus,
    type CrossFolderCategory,
    type DeleteDuplicateResult,
    type Findings
  } from '$lib/api/integrity';
  import { formatBytes } from '$lib/format';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import DeleteCopyButton from '$lib/components/DeleteCopyButton.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import EvidenceRow from '$lib/components/EvidenceRow.svelte';
  import IntegritySection from '$lib/components/IntegritySection.svelte';

  let { data } = $props();
  let findings = $state<Findings>(data.findings);
  let analyze = $state<AnalyzeStatus>(data.analyze);
  let error = $state<ApiError | null>(null);
  let starting = $state(false);

  const classesFor = (id: number) => findings.classes_per_file[String(id)];

  const recon = $derived(findings.reconciliation);
  const conclusive = $derived(walkIsConclusive(recon));
  const reconCount = $derived(
    recon.orphans.length + recon.ghosts.length + recon.present_but_marked_absent.length
  );

  // Content duplicates and cross-series both depend on digests, so both must
  // say when they haven't looked rather than showing a bare 0.
  const pending = $derived(findings.unanalyzed_candidates);
  const analysisNote = $derived(
    pending > 0 ? `${pending} file${pending === 1 ? '' : 's'} not yet analyzed` : undefined
  );
  const crossSeries = $derived(findings.content_duplicates.filter((g) => g.spans_multiple_series));
  const reclaimable = $derived(
    findings.content_duplicates.reduce((n, g) => n + g.redundant_bytes, 0)
  );

  const byCategory = (c: CrossFolderCategory) =>
    findings.cross_folder.filter((f) => f.category === c);

  async function refresh(): Promise<void> {
    try {
      [findings, analyze] = await Promise.all([getFindings(), getAnalyzeStatus()]);
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    }
  }

  // What happened is read from the RESPONSE, never assumed from the
  // delete having succeeded. `describeRevert` picks the sentence from
  // `now_missing` and `search_outlook`; the one live counter-example is
  // an issue that owns a second file outside the group and therefore
  // does NOT revert.
  //
  // A failed unlink is reported separately and loudly: in that case the
  // catalog row is already gone and the bytes are still on disk, so the
  // file is now an orphan. Saying only "deleted" there would be a lie
  // by omission about the state of the disk.
  async function afterDelete(result: DeleteDuplicateResult): Promise<void> {
    if (result.unlink_error) {
      toast.error(
        `The catalog entry was removed but the file is still on disk — it will show up as an orphan on the next scan. ${result.unlink_error}`
      );
    } else {
      toast.success(describeRevert(result.reverted));
    }
    // Re-read rather than splice the group locally: deleting a copy can
    // drop a group below two files and remove it from the findings
    // entirely, and can change the reclaimable total. Recomputing that
    // in the client would be a second implementation of the detector.
    await refresh();
  }

  async function runAnalyze(): Promise<void> {
    starting = true;
    error = null;
    try {
      await startAnalyze();
      analyze = { ...analyze, running: true };
      toast.success('Content analysis started.');
      void poll();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      starting = false;
    }
  }

  // The pass is spawned server-side, so the status endpoint is the only way
  // to observe completion.
  async function poll(): Promise<void> {
    for (let i = 0; i < 600; i++) {
      await new Promise((r) => setTimeout(r, 1000));
      try {
        analyze = await getAnalyzeStatus();
      } catch {
        break;
      }
      if (!analyze.running) {
        await refresh();
        const s = analyze.last;
        if (s) {
          toast.success(
            `Analyzed ${s.hashed} file${s.hashed === 1 ? '' : 's'}` +
              (s.failed > 0 ? `, ${s.failed} failed` : '') +
              '.'
          );
        }
        return;
      }
    }
  }
</script>

<svelte:head><title>Library integrity · LongBox</title></svelte:head>

<div class="mx-auto max-w-5xl space-y-3 p-4">
  <header class="flex flex-wrap items-baseline justify-between gap-2">
    <h1 class="flex items-center gap-2 text-lg font-semibold">
      <FileSearch class="size-5 text-slate-500" aria-hidden="true" />
      Library integrity
    </h1>
    <Button size="sm" variant="ghost" onclick={refresh}>Refresh</Button>
  </header>

  <p class="text-sm text-slate-600">
    Nothing on this page deletes, moves or re-points a file yet — it reports what it found so you
    can decide. <strong>Analyze content</strong> reads files and records their checksums; it changes
    no bindings and no files on disk.
  </p>

  {#if error}
    <ErrorBanner {error} onDismiss={() => (error = null)} />
  {/if}

  <!-- (a) content duplicates — the class Tidy structurally cannot see -->
  <IntegritySection
    title="Duplicate content"
    count={findings.content_duplicates.length}
    note={analysisNote}
    warn
    open={true}
  >
    <div class="mb-3 flex flex-wrap items-center gap-3">
      <Button size="sm" disabled={analyze.running || starting} onclick={runAnalyze}>
        {#if analyze.running}
          <RefreshCw class="mr-1 size-3 animate-spin" aria-hidden="true" /> Analyzing…
        {:else}
          Analyze content
        {/if}
      </Button>
      {#if pending > 0}
        <span class="text-sm text-amber-800">
          {pending} file{pending === 1 ? '' : 's'} share a size but haven’t been compared
          byte-for-byte. Until they are, the count above is a floor, not a total.
        </span>
      {:else}
        <span class="text-sm text-slate-500">
          Every file that could have a twin has been compared.
        </span>
      {/if}
    </div>
    {#if analyze.last_error}
      <p class="mb-2 text-sm text-red-800">Last analysis failed: {analyze.last_error}</p>
    {:else if analyze.last?.first_failure}
      <p class="mb-2 text-sm text-amber-800">
        {analyze.last.failed} file{analyze.last.failed === 1 ? '' : 's'} could not be read. First: {analyze
          .last.first_failure}
      </p>
    {/if}

    {#if findings.content_duplicates.length === 0}
      <p class="text-sm text-slate-500">
        {pending > 0
          ? 'No duplicates found yet — run the analysis above.'
          : 'No files in this library share their bytes with another. 🎉'}
      </p>
    {:else}
      <p class="mb-2 text-sm text-slate-600">
        Files whose bytes are identical, wherever they sit. Library Tidy can’t see these: it groups
        by issue, and most of these span different issues — which means an issue is marked owned
        while holding another issue’s content. {formatBytes(reclaimable)} is redundant.
      </p>
      <ul class="space-y-2">
        {#each findings.content_duplicates as g (g.digest)}
          <li class="rounded-md border border-slate-200">
            <div class="flex flex-wrap items-baseline gap-2 border-b border-slate-100 px-3 py-2">
              <span class="text-sm font-medium">{g.files.length} identical copies</span>
              <span class="text-xs text-slate-500">{formatBytes(g.size_bytes)} each</span>
              <span class="text-xs text-slate-500">
                · {formatBytes(g.redundant_bytes)} redundant
              </span>
              {#if g.distinct_issue_ids.length > 1}
                <span
                  class="rounded bg-amber-50 px-1.5 py-0.5 text-xs font-medium text-amber-800"
                >
                  {g.distinct_issue_ids.length} different issues
                </span>
              {/if}
              {#if g.spans_multiple_series}
                <span class="rounded bg-red-50 px-1.5 py-0.5 text-xs font-medium text-red-800">
                  two series
                </span>
              {/if}
              <span class="ml-auto font-mono text-[11px] text-slate-400">
                {g.digest.slice(0, 12)}
              </span>
            </div>
            <ul class="divide-y divide-slate-100">
              {#each g.files as f (f.file_id)}
                <EvidenceRow file={f} classes={classesFor(f.file_id)}>
                  {#snippet action()}
                    <DeleteCopyButton
                      digest={g.digest}
                      file={f}
                      groupSize={g.files.length}
                      onDeleted={afterDelete}
                    />
                  {/snippet}
                </EvidenceRow>
              {/each}
            </ul>
          </li>
        {/each}
      </ul>
    {/if}
  </IntegritySection>

  <!-- (e) identical content under two series -->
  <IntegritySection
    title="Same file under two series"
    count={crossSeries.length}
    note={analysisNote}
    warn
  >
    <p class="mb-2 text-sm text-slate-600">
      One comic filed under two different series. The catalog believes it owns both, and opening
      one of them serves the other’s content. The archive’s own internal name usually says which
      series it really is — compare the “archive says” column below.
    </p>
    <!-- No delete control here on purpose. These are the same groups the
         section above lists, shown again because crossing a series
         boundary is worth its own heading. Repeating the action would
         put two delete buttons on one file in two places, and the user
         would have no way to tell they were the same decision. -->
    {#if crossSeries.length === 0}
      <p class="text-sm text-slate-500">
        {pending > 0
          ? 'Not determined yet — this depends on the content analysis above.'
          : 'No file is filed under two series.'}
      </p>
    {:else}
      <ul class="space-y-2">
        {#each crossSeries as g (g.digest)}
          <li class="rounded-md border border-red-200">
            <ul class="divide-y divide-slate-100">
              {#each g.files as f (f.file_id)}
                <EvidenceRow file={f} classes={classesFor(f.file_id)} />
              {/each}
            </ul>
          </li>
        {/each}
      </ul>
    {/if}
  </IntegritySection>

  <!-- (b) disk/DB reconciliation -->
  <IntegritySection
    title="Disk and catalog disagree"
    count={reconCount}
    note={conclusive ? undefined : 'walk incomplete'}
    warn
  >
    <div class="mb-3 rounded border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600">
      Walked <span class="font-mono">{recon.provenance.root}</span> ·
      {recon.provenance.files_seen.toLocaleString()} comic files seen ·
      {recon.provenance.rows_compared.toLocaleString()} catalog rows compared ·
      {recon.provenance.duration_ms} ms
      {#if !conclusive}
        <span class="font-medium text-amber-800">
          — incomplete, so the counts below are a floor
          {#if recon.provenance.unreadable.length > 0}
            ({recon.provenance.unreadable.length} unreadable){/if}
        </span>
      {/if}
    </div>
    {#if reconCount === 0}
      <p class="text-sm text-slate-500">
        {conclusive
          ? 'Every file on disk has a catalog row, and every row that claims a file has one.'
          : 'Nothing found — but the walk did not complete, so this is not a clean bill of health.'}
      </p>
    {:else}
      {#each [['On disk, not in the catalog', recon.orphans], ['Catalog says present, nothing on disk', recon.ghosts], ['Catalog says absent, but the file is there', recon.present_but_marked_absent]] as [label, paths]}
        {#if (paths as string[]).length > 0}
          <p class="mt-2 text-sm font-medium">{label} ({(paths as string[]).length})</p>
          <ul class="mt-1 space-y-0.5">
            {#each paths as string[] as p (p)}
              <li class="font-mono text-xs break-all text-slate-700">{p}</li>
            {/each}
          </ul>
        {/if}
      {/each}
    {/if}
  </IntegritySection>

  <!-- (c) cross-folder, three categories -->
  <IntegritySection title="Files outside their series folder" count={findings.cross_folder.length}>
    {#each [['wrong_volume', 'Bound to the wrong volume', 'The folder names a different volume of this title that the catalog also has. This is the shape that bound 26 Authority issues to the wrong volume.'], ['trade_or_collection', 'Collected edition or differently-titled folder', 'Needs a human call — these are usually trades or omnibuses holding a series’ issues.'], ['benign_variant', 'Same series, differently-spelled folder', 'Cosmetic. Nothing is wrong with the binding.']] as [cat, label, blurb]}
      {@const rows = byCategory(cat as CrossFolderCategory)}
      <div class="mb-3">
        <p class="text-sm font-medium">
          {label}
          <span class="ml-1 rounded bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600">
            {rows.length}
          </span>
          {#if rows.length === 0}
            <span class="ml-1 text-xs font-normal text-slate-500">none found</span>
          {/if}
        </p>
        <p class="text-xs text-slate-500">{blurb}</p>
        {#if rows.length > 0}
          <ul class="mt-1 divide-y divide-slate-100 rounded border border-slate-100">
            {#each rows as r (r.file.file_id)}
              <li class="px-3 py-2 text-sm">
                <div class="font-mono text-xs break-all text-slate-700">
                  {r.file.path_relative}
                </div>
                <div class="text-xs text-slate-500">
                  most of <span class="text-slate-700">{r.file.series_title}</span> lives in
                  <span class="font-mono">{r.main_folder}</span>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/each}
  </IntegritySection>

  <!-- (d) filename vs assigned issue -->
  <IntegritySection
    title="Filename disagrees with the issue it’s filed under"
    count={findings.filename_disagreements.length}
    warn
  >
    <p class="mb-2 text-sm text-slate-600">
      The filename names one issue; the catalog has it under another. One of the two is wrong, and
      the archive and ComicInfo columns are the tiebreakers.
    </p>
    {#if findings.filename_disagreements.length === 0}
      <p class="text-sm text-slate-500">Every filename agrees with its issue.</p>
    {:else}
      <ul class="divide-y divide-slate-100 rounded border border-slate-100">
        {#each findings.filename_disagreements as d (d.file.file_id)}
          <li>
            <div class="px-3 pt-2 text-sm">
              filename says <span class="font-medium">#{d.filename_says}</span>, filed under
              <span class="font-medium">#{d.bound_to}</span>
            </div>
            <EvidenceRow file={d.file} classes={classesFor(d.file.file_id)} />
          </li>
        {/each}
      </ul>
    {/if}
  </IntegritySection>

  <!-- (f) orphaned owned rows — recorded, no action in this release -->
  <IntegritySection title="Catalog rows pointing at nothing" count={findings.orphaned_owned_rows.length}>
    <p class="mb-2 text-sm text-slate-600">
      Rows marked owned with no issue attached. They are absent from disk and unreachable by a
      rematch, so they sit in the catalog doing nothing. They don’t affect your owned counts or
      your missing-issue list — <strong>recorded here, resolution ships in the next release</strong
      >.
    </p>
    {#if findings.orphaned_owned_rows.length > 0}
      <ul class="max-h-96 divide-y divide-slate-100 overflow-y-auto rounded border border-slate-100">
        {#each findings.orphaned_owned_rows as r (r.file_id)}
          <li class="px-3 py-1.5">
            <span class="font-mono text-xs break-all text-slate-700">{r.path_relative}</span>
            <span class="ml-2 text-xs text-slate-400">{r.match_method}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </IntegritySection>
</div>
