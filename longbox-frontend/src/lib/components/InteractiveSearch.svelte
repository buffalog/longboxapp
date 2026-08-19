<script lang="ts">
  // Interactive Search modal — ad-hoc NZB search for one issue. Query is
  // pre-populated (series title + issue number) and editable; results are
  // returned for ALL confidences (the user decides) and grabbed per-row.
  import { Search, Download, Loader2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    interactiveSearch,
    grabRelease,
    type InteractiveSearchResult,
    type IndexerSearchError
  } from '$lib/api/interactiveSearch';
  import { formatBytes, formatConfidence } from '$lib/format';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from './Button.svelte';
  import Modal from './Modal.svelte';

  interface Props {
    open: boolean;
    onClose: () => void;
    seriesId: number;
    issueId: number;
    seriesTitle: string;
    issueNumber: string;
  }

  let { open, onClose, seriesId, issueId, seriesTitle, issueNumber }: Props = $props();

  let query = $state('');
  let results = $state<InteractiveSearchResult[]>([]);
  let errors = $state<IndexerSearchError[]>([]);
  let alreadyOwned = $state(false);
  let searching = $state(false);
  let searched = $state(false);
  // The nzb_url currently being grabbed (per-row spinner / disable).
  let grabbing = $state<string | null>(null);

  type SortKey = 'title' | 'indexer' | 'size_bytes' | 'age_days' | 'confidence';
  let sortKey = $state<SortKey>('confidence');
  let sortDir = $state<'asc' | 'desc'>('desc');

  // Re-seed the query and clear prior results whenever the modal opens (or
  // is reused for a different issue). The query box stays editable after.
  $effect(() => {
    if (open) {
      query = `${seriesTitle} ${issueNumber}`.trim();
      results = [];
      errors = [];
      alreadyOwned = false;
      searched = false;
      sortKey = 'confidence';
      sortDir = 'desc';
    }
  });

  const sortedResults = $derived.by(() => {
    const sign = sortDir === 'asc' ? 1 : -1;
    return [...results].sort((a, b) => {
      const av = a[sortKey];
      const bv = b[sortKey];
      // Nulls (size/age may be absent) always sort to the bottom.
      if (av === null && bv === null) return 0;
      if (av === null) return 1;
      if (bv === null) return -1;
      if (typeof av === 'string' && typeof bv === 'string') {
        return sign * av.localeCompare(bv);
      }
      return sign * (Number(av) - Number(bv));
    });
  });

  function toggleSort(key: SortKey): void {
    if (sortKey === key) {
      sortDir = sortDir === 'asc' ? 'desc' : 'asc';
    } else {
      sortKey = key;
      // Text columns default ascending; numeric default descending.
      sortDir = key === 'title' || key === 'indexer' ? 'asc' : 'desc';
    }
  }

  function confidenceClass(c: number): string {
    if (c >= 0.75) return 'text-emerald-700';
    if (c >= 0.5) return 'text-amber-600';
    return 'text-red-600';
  }

  async function runSearch(): Promise<void> {
    if (searching || query.trim() === '') return;
    searching = true;
    try {
      const res = await interactiveSearch({
        series_id: seriesId,
        issue_id: issueId,
        query: query.trim()
      });
      results = res.results;
      errors = res.errors;
      alreadyOwned = res.already_owned;
      searched = true;
    } catch (e) {
      toast.warning(e instanceof ApiError ? e.message : 'Search failed.');
    } finally {
      searching = false;
    }
  }

  async function grab(r: InteractiveSearchResult): Promise<void> {
    if (grabbing !== null) return;
    grabbing = r.nzb_url;
    try {
      const res = await grabRelease({ nzb_url: r.nzb_url, title: r.title });
      if (res.success) {
        toast.success(`Sent “${r.title}” to the downloader.`);
        onClose();
      } else {
        toast.warning(res.error ?? 'The downloader rejected the release.');
      }
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : 'Grab failed.');
    } finally {
      grabbing = null;
    }
  }
</script>

<Modal {open} {onClose} maxWidth="max-w-4xl" title={`Interactive Search — ${seriesTitle} #${issueNumber}`}>
  <form
    class="mb-3 flex gap-2"
    onsubmit={(e) => {
      e.preventDefault();
      void runSearch();
    }}
  >
    <input
      type="text"
      bind:value={query}
      disabled={searching}
      aria-label="Search query"
      class="flex-1 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
    />
    <Button type="submit" loading={searching} disabled={searching || query.trim() === ''}>
      <Search class="size-4" aria-hidden="true" />
      Search
    </Button>
  </form>

  <div role="status" aria-live="polite">
    {#if alreadyOwned}
      <div
        class="mb-3 rounded-md border border-slate-300 bg-slate-100 px-3 py-2 text-xs text-slate-700"
      >
        <span class="font-medium">Note — you already have this issue.</span> This page loaded
        before the file appeared; reload to see it. Grabbing again will not replace it: when a
        download collides with a file already at the issue's library path, it is discarded and the
        existing file is kept. Delete the old file first if you mean to replace it.
      </div>
    {/if}
  </div>

  {#if errors.length > 0}
    <div class="mb-3 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-800">
      {#each errors as err (err.indexer)}
        <div><span class="font-medium">{err.indexer}:</span> {err.error}</div>
      {/each}
    </div>
  {/if}

  {#if sortedResults.length > 0}
    <div class="max-h-[60vh] overflow-auto rounded-md border border-slate-200">
      <table class="w-full text-sm">
        <thead class="sticky top-0 bg-slate-50 text-left text-xs text-slate-500">
          <tr>
            {#each [['title', 'Title'], ['indexer', 'Indexer'], ['size_bytes', 'Size'], ['age_days', 'Age'], ['confidence', 'Confidence']] as [key, label] (key)}
              <th
                class="px-3 py-2 font-medium"
                aria-sort={sortKey === key ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none'}
              >
                <button
                  type="button"
                  class="inline-flex items-center gap-1 hover:text-slate-900"
                  onclick={() => toggleSort(key as SortKey)}
                >
                  {label}
                  {#if sortKey === key}<span aria-hidden="true">{sortDir === 'asc' ? '▲' : '▼'}</span>{/if}
                </button>
              </th>
            {/each}
            <th class="px-3 py-2 font-medium">Grab</th>
          </tr>
        </thead>
        <tbody>
          {#each sortedResults as r (r.nzb_url)}
            <tr class="border-t border-slate-100">
              <td class="max-w-md px-3 py-2"><span class="block truncate" title={r.title}>{r.title}</span></td>
              <td class="whitespace-nowrap px-3 py-2 text-slate-600">{r.indexer}</td>
              <td class="whitespace-nowrap px-3 py-2 tabular-nums text-slate-600">
                {r.size_bytes !== null ? formatBytes(r.size_bytes) : '—'}
              </td>
              <td class="whitespace-nowrap px-3 py-2 tabular-nums text-slate-600">
                {r.age_days !== null ? `${r.age_days}d` : '—'}
              </td>
              <td class="whitespace-nowrap px-3 py-2 font-medium tabular-nums {confidenceClass(r.confidence)}">
                {formatConfidence(r.confidence)}
              </td>
              <td class="px-3 py-2">
                <Button
                  type="button"
                  size="sm"
                  onclick={() => grab(r)}
                  disabled={grabbing !== null}
                  loading={grabbing === r.nzb_url}
                >
                  {#if grabbing !== r.nzb_url}<Download class="size-3.5" aria-hidden="true" />{/if}
                  Grab
                </Button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if searching}
    <div class="flex items-center justify-center gap-2 py-8 text-sm text-slate-500">
      <Loader2 class="size-4 animate-spin" aria-hidden="true" /> Searching indexers…
    </div>
  {:else if searched}
    <p class="py-8 text-center text-sm text-slate-500">
      No results from any indexer for “{query}”. Edit the query and try again.
    </p>
  {:else}
    <p class="py-8 text-center text-sm text-slate-500">
      Press Search to query your indexers.
    </p>
  {/if}
</Modal>
