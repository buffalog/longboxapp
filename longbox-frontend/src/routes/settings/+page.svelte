<script lang="ts">
  // Phase A settings UI is read-only for env vars. Editing requires
  // restarting the backend binary; values shown here are reflected from
  // /api/settings. Publisher filters are the one editable surface.
  import { invalidateAll } from '$app/navigation';
  import { Trash2, RotateCcw, Power } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { restartServer } from '$lib/api/admin';
  import { addFilter, deleteFilter, resetFiltersToDefaults } from '$lib/api/publishers';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import DownloaderSettings from '$lib/components/DownloaderSettings.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import IndexerSettings from '$lib/components/IndexerSettings.svelte';
  import WebhookSettings from '$lib/components/WebhookSettings.svelte';

  let { data } = $props();
  const s = $derived(data.settings);

  let newPublisher = $state('');
  let busy = $state(false);
  let error = $state<ApiError | null>(null);

  interface Row {
    label: string;
    value: string;
    envVar: string;
    mono?: boolean;
  }

  const rows = $derived<Row[]>([
    {
      label: 'Library root',
      value: s.library_root_path,
      envVar: 'LIBRARY_ROOT_PATH',
      mono: true
    },
    {
      label: 'Database',
      value: s.database_url,
      envVar: 'DATABASE_URL',
      mono: true
    },
    {
      label: 'Bind address',
      value: s.bind_address,
      envVar: 'BIND_ADDR',
      mono: true
    },
    {
      label: 'Match threshold',
      value: s.match_threshold.toFixed(2),
      envVar: 'MATCH_THRESHOLD'
    },
    {
      label: 'Log level',
      value: s.log_level,
      envVar: 'LOG_LEVEL'
    }
  ]);

  async function withBusy(fn: () => Promise<unknown>): Promise<void> {
    busy = true;
    error = null;
    try {
      await fn();
      await invalidateAll();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      busy = false;
    }
  }

  async function handleAdd(): Promise<void> {
    const name = newPublisher.trim();
    if (!name) return;
    await withBusy(async () => {
      await addFilter(name);
      newPublisher = '';
    });
  }

  function handleDelete(id: number): Promise<void> {
    return withBusy(() => deleteFilter(id));
  }

  function handleReset(): Promise<void> {
    return withBusy(() => resetFiltersToDefaults());
  }

  let restarting = $state(false);

  // The server responds 202 and exits ~500ms later; Docker's
  // `restart: unless-stopped` brings it back up. Wait 4s before
  // reloading — enough cushion for the container to recreate and the
  // healthcheck to pass on most hardware.
  async function handleRestart(): Promise<void> {
    if (restarting) return;
    restarting = true;
    try {
      await restartServer();
      toast.success('Restarting… page will reload automatically.');
      setTimeout(() => window.location.reload(), 4000);
    } catch (e) {
      restarting = false;
      const message = e instanceof ApiError ? e.message : 'Could not restart LongBox.';
      toast.warning(message);
    }
    // Deliberately leave `restarting = true` on success — the page is
    // about to reload, no point flickering the spinner off.
  }
</script>

<h1 class="mb-4 text-2xl font-bold">Settings</h1>

<div class="space-y-4">
  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">Configuration</h2>
    <p class="mb-3 text-sm text-slate-600">
      LongBox reads its settings from environment variables at startup. The values are not
      editable from the UI in Phase A — restart the binary with different env vars to change them.
    </p>
    <dl class="grid grid-cols-1 gap-x-4 gap-y-3 text-sm sm:grid-cols-[10rem_1fr]">
      {#each rows as r (r.envVar)}
        <dt class="font-medium text-slate-700">{r.label}</dt>
        <dd>
          <span class={r.mono ? 'font-mono text-slate-900' : 'text-slate-900'}>{r.value}</span>
          <span class="ml-2 text-xs text-slate-500">set via <code>{r.envVar}</code></span>
        </dd>
      {/each}

      <dt class="font-medium text-slate-700">ComicVine API</dt>
      <dd>
        {#if s.comicvine_api_key_configured}
          <span class="text-emerald-700">Configured</span>
        {:else}
          <span class="text-red-700">Not configured</span>
        {/if}
        <span class="ml-2 text-xs text-slate-500">
          set via <code>COMICVINE_API_KEY</code> (value never displayed)
        </span>
      </dd>

      <dt class="font-medium text-slate-700">Watch folder</dt>
      <dd>
        {#if s.download_watch_path}
          <span class="font-mono text-slate-900">{s.download_watch_path}</span>
          <span class="ml-2 text-xs text-emerald-700">(Phase B enabled)</span>
        {:else}
          <span class="text-slate-500">— (Phase B disabled)</span>
        {/if}
        <span class="ml-2 text-xs text-slate-500">set via <code>DOWNLOAD_WATCH_PATH</code></span>
      </dd>

      <dt class="font-medium text-slate-700">Version</dt>
      <dd>
        <span class="font-mono text-slate-900">{s.version}</span>
      </dd>
    </dl>
  </section>

  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <header class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
      <h2 class="text-base font-semibold">Publisher filters</h2>
      <Button variant="ghost" size="sm" onclick={handleReset} disabled={busy}>
        <RotateCcw class="size-3.5" aria-hidden="true" />Reset to defaults
      </Button>
    </header>
    <p class="mb-3 text-sm text-slate-600">
      ComicVine search excludes results whose publisher matches any name below.
      Reprint publishers like Panini and Planeta are blocked by default so a
      search for "Batman" returns the DC original, not the French/Spanish
      reprints. Add your own, remove any that aren't useful.
    </p>

    {#if error}
      <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
    {/if}

    <form
      class="mb-3 flex gap-2"
      onsubmit={(e) => {
        e.preventDefault();
        void handleAdd();
      }}
    >
      <input
        type="text"
        bind:value={newPublisher}
        placeholder="Publisher name (case-insensitive)"
        class="flex-1 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        disabled={busy}
      />
      <Button type="submit" disabled={busy || newPublisher.trim() === ''}>Add</Button>
    </form>

    {#if data.publisherFilters.length === 0}
      <p class="text-sm text-slate-500">
        No publisher filters. Click "Reset to defaults" to restore the curated blocklist.
      </p>
    {:else}
      <ul class="divide-y divide-slate-100 rounded-md border border-slate-200">
        {#each data.publisherFilters as f (f.id)}
          <li class="flex items-center justify-between gap-3 px-3 py-1.5 text-sm">
            <span>{f.publisher_name}</span>
            <button
              type="button"
              class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-red-700 focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:opacity-50"
              aria-label={`Remove filter ${f.publisher_name}`}
              onclick={() => handleDelete(f.id)}
              disabled={busy}
            >
              <Trash2 class="size-4" aria-hidden="true" />
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <IndexerSettings indexers={data.indexers} />

  <DownloaderSettings downloader={data.downloader} />

  <WebhookSettings webhooks={data.webhooks} />

  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">About</h2>
    <p class="text-sm text-slate-600">
      LongBox is a self-hosted comic library catalog. Phase A is the foundation: tracks watched
      series, matches files on disk against issues, surfaces what's owned and what's missing. No
      downloading or file mutation.
    </p>
  </section>

  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">System</h2>
    <p class="mb-3 text-sm text-slate-600">
      Restart LongBox to pick up new environment variables or recover from a stuck worker. The
      container exits and Docker brings it back up — in-flight requests will fail and any
      running scans or sweeps will be cut short.
    </p>
    <Button variant="warning" onclick={handleRestart} loading={restarting} disabled={restarting}>
      <Power class="size-4" aria-hidden="true" />
      {restarting ? 'Restarting…' : 'Restart LongBox'}
    </Button>
  </section>
</div>
