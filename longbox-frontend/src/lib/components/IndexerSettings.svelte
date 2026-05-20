<script lang="ts">
  // Newznab indexer CRUD section of the Settings page.
  //
  // Self-contained: seeded once from the page's load data, then it
  // maintains its own `$state` list from each mutation's response.
  // Nothing outside this component mutates indexer data, so seed-once
  // is correct and keeps the component testable in isolation.
  import { Pencil, Trash2 } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import {
    createIndexer,
    deleteIndexer,
    testIndexer,
    updateIndexer,
    type ConnectionTestResult,
    type Indexer,
    type IndexerInput
  } from '$lib/api/indexers';
  import Button from './Button.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  let { indexers }: { indexers: Indexer[] } = $props();

  let items = $state<Indexer[]>([...indexers]);
  let busy = $state(false);
  let error = $state<ApiError | null>(null);

  /** Per-row connection-test results, keyed by indexer id. */
  let testResults = $state<Record<number, ConnectionTestResult>>({});
  let addTestResult = $state<ConnectionTestResult | null>(null);

  let showAdd = $state(false);
  let addForm = $state<IndexerInput>(blankForm());
  let editingId = $state<number | null>(null);
  let editForm = $state<IndexerInput>(blankForm());

  function blankForm(): IndexerInput {
    return { name: '', base_url: '', api_key: '', enabled: true, priority: 0, maxage_days: 1500 };
  }

  async function run(fn: () => Promise<void>): Promise<void> {
    busy = true;
    error = null;
    try {
      await fn();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      busy = false;
    }
  }

  function handleAdd(): Promise<void> {
    return run(async () => {
      const created = await createIndexer(addForm);
      items = [...items, created];
      addForm = blankForm();
      addTestResult = null;
      showAdd = false;
    });
  }

  function startEdit(ix: Indexer): void {
    editingId = ix.id;
    // api_key starts blank — the placeholder explains that blank keeps
    // the stored key.
    editForm = {
      name: ix.name,
      base_url: ix.base_url,
      api_key: '',
      enabled: ix.enabled,
      priority: ix.priority,
      maxage_days: ix.maxage_days
    };
  }

  function handleSaveEdit(id: number): Promise<void> {
    return run(async () => {
      const updated = await updateIndexer(id, editForm);
      items = items.map((i) => (i.id === id ? updated : i));
      editingId = null;
    });
  }

  function handleDelete(id: number): Promise<void> {
    return run(async () => {
      await deleteIndexer(id);
      items = items.filter((i) => i.id !== id);
    });
  }

  function handleTestAdd(): Promise<void> {
    return run(async () => {
      addTestResult = await testIndexer({
        name: addForm.name,
        base_url: addForm.base_url,
        api_key: addForm.api_key,
        maxage_days: addForm.maxage_days
      });
    });
  }

  function handleTestRow(ix: Indexer): Promise<void> {
    return run(async () => {
      // While editing a row, test the in-progress key; otherwise send a
      // blank key so the server re-uses the stored one.
      const result = await testIndexer({
        id: ix.id,
        name: ix.name,
        base_url: editingId === ix.id ? editForm.base_url : ix.base_url,
        api_key: editingId === ix.id ? editForm.api_key : '',
        maxage_days: ix.maxage_days
      });
      testResults = { ...testResults, [ix.id]: result };
    });
  }
</script>

{#snippet fields(form: IndexerInput, keyPlaceholder: string)}
  <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
    <label class="text-xs font-medium text-slate-600">
      Name
      <input
        type="text"
        bind:value={form.name}
        disabled={busy}
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>
    <label class="text-xs font-medium text-slate-600">
      Base URL
      <input
        type="url"
        bind:value={form.base_url}
        disabled={busy}
        placeholder="https://api.nzbgeek.info"
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>
    <label class="text-xs font-medium text-slate-600">
      API key
      <input
        type="password"
        bind:value={form.api_key}
        disabled={busy}
        placeholder={keyPlaceholder}
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>
    <div class="grid grid-cols-2 gap-2">
      <label class="text-xs font-medium text-slate-600">
        Priority
        <input
          type="number"
          bind:value={form.priority}
          disabled={busy}
          class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        />
      </label>
      <label class="text-xs font-medium text-slate-600">
        Max age (days)
        <input
          type="number"
          bind:value={form.maxage_days}
          disabled={busy}
          class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        />
      </label>
    </div>
    <label class="flex items-center gap-2 text-sm text-slate-700">
      <input type="checkbox" bind:checked={form.enabled} disabled={busy} />
      Enabled
    </label>
  </div>
{/snippet}

{#snippet testLine(result: ConnectionTestResult)}
  <p class="mt-1 text-xs {result.ok ? 'text-emerald-700' : 'text-red-700'}">
    {result.ok ? '✓ ' : '✗ '}{result.message}
  </p>
{/snippet}

<section class="rounded-lg border border-slate-200 bg-white p-4">
  <header class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
    <h2 class="text-base font-semibold">Indexers</h2>
    <Button
      variant="ghost"
      size="sm"
      onclick={() => {
        showAdd = !showAdd;
        addTestResult = null;
      }}
      disabled={busy}
    >
      {showAdd ? 'Cancel' : '+ Add indexer'}
    </Button>
  </header>
  <p class="mb-3 text-sm text-slate-600">
    Newznab indexers searched for new issues, lowest priority number first.
  </p>

  {#if error}
    <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
  {/if}

  {#if showAdd}
    <form
      class="mb-3 rounded-md border border-slate-200 bg-slate-50 p-3"
      onsubmit={(e) => {
        e.preventDefault();
        void handleAdd();
      }}
    >
      {@render fields(addForm, 'Required')}
      {#if addTestResult}{@render testLine(addTestResult)}{/if}
      <div class="mt-2 flex gap-2">
        <Button type="submit" disabled={busy || addForm.name.trim() === ''}>Add indexer</Button>
        <Button
          type="button"
          variant="secondary"
          onclick={handleTestAdd}
          disabled={busy || addForm.base_url.trim() === ''}
        >
          Test connection
        </Button>
      </div>
    </form>
  {/if}

  {#if items.length === 0}
    <p class="text-sm text-slate-500">No indexers configured.</p>
  {:else}
    <ul class="divide-y divide-slate-100 rounded-md border border-slate-200">
      {#each items as ix (ix.id)}
        {@const rowTest = testResults[ix.id]}
        <li class="px-3 py-2 text-sm">
          {#if editingId === ix.id}
            <form
              onsubmit={(e) => {
                e.preventDefault();
                void handleSaveEdit(ix.id);
              }}
            >
              {@render fields(editForm, 'Leave blank to keep current key')}
              <div class="mt-2 flex gap-2">
                <Button type="submit" disabled={busy || editForm.name.trim() === ''}>Save</Button>
                <Button
                  type="button"
                  variant="secondary"
                  onclick={() => handleTestRow(ix)}
                  disabled={busy}
                >
                  Test connection
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onclick={() => (editingId = null)}
                  disabled={busy}
                >
                  Cancel
                </Button>
              </div>
            </form>
          {:else}
            <div class="flex items-center justify-between gap-3">
              <div class="min-w-0">
                <span class="font-medium">{ix.name}</span>
                {#if !ix.enabled}
                  <span class="ml-1 text-xs text-slate-400">(disabled)</span>
                {/if}
                <span class="ml-2 text-xs text-slate-500">priority {ix.priority}</span>
                <div class="truncate text-xs text-slate-500">{ix.base_url}</div>
              </div>
              <div class="flex flex-shrink-0 items-center gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  onclick={() => handleTestRow(ix)}
                  disabled={busy}
                >
                  Test
                </Button>
                <button
                  type="button"
                  class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:opacity-50"
                  aria-label={`Edit indexer ${ix.name}`}
                  onclick={() => startEdit(ix)}
                  disabled={busy}
                >
                  <Pencil class="size-4" aria-hidden="true" />
                </button>
                <button
                  type="button"
                  class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-red-700 focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:opacity-50"
                  aria-label={`Remove indexer ${ix.name}`}
                  onclick={() => handleDelete(ix.id)}
                  disabled={busy}
                >
                  <Trash2 class="size-4" aria-hidden="true" />
                </button>
              </div>
            </div>
          {/if}
          {#if rowTest}{@render testLine(rowTest)}{/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>
