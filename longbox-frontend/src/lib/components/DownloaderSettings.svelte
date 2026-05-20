<script lang="ts">
  // Usenet downloader configuration — a single-row config, so this is
  // one form rather than a list. Self-contained: seeded from the page's
  // load data, then maintained from each mutation's response.
  import { ApiError } from '$lib/api/client';
  import {
    clearDownloader,
    saveDownloader,
    testDownloader,
    type ConnectionTestResult,
    type Downloader,
    type DownloaderInput
  } from '$lib/api/downloader';
  import Button from './Button.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  let { downloader }: { downloader: Downloader | null } = $props();

  /** Form state — `username` is always a string here ('' for SAB);
   *  the server ignores it for SAB and requires it for NZBGet. */
  interface Form {
    kind: 'sab' | 'nzbget';
    base_url: string;
    username: string;
    secret: string;
    category: string;
    enabled: boolean;
  }

  function toForm(d: Downloader | null): Form {
    return d
      ? {
          kind: d.kind,
          base_url: d.base_url,
          username: d.username ?? '',
          secret: '',
          category: d.category,
          enabled: d.enabled
        }
      : { kind: 'sab', base_url: '', username: '', secret: '', category: '', enabled: true };
  }

  let current = $state<Downloader | null>(downloader);
  let form = $state<Form>(toForm(downloader));
  let busy = $state(false);
  let error = $state<ApiError | null>(null);
  let testResult = $state<ConnectionTestResult | null>(null);

  /** A stored secret governs the "leave blank to keep" placeholder. */
  const hasStoredSecret = $derived(current?.has_secret ?? false);

  function payload(): DownloaderInput {
    return {
      kind: form.kind,
      base_url: form.base_url,
      username: form.kind === 'nzbget' ? form.username : null,
      secret: form.secret,
      category: form.category,
      enabled: form.enabled
    };
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

  function handleSave(): Promise<void> {
    return run(async () => {
      const saved = await saveDownloader(payload());
      current = saved;
      form = toForm(saved);
      testResult = null;
    });
  }

  function handleTest(): Promise<void> {
    return run(async () => {
      testResult = await testDownloader(payload());
    });
  }

  function handleClear(): Promise<void> {
    return run(async () => {
      await clearDownloader();
      current = null;
      form = toForm(null);
      testResult = null;
    });
  }
</script>

<section class="rounded-lg border border-slate-200 bg-white p-4">
  <h2 class="mb-2 text-base font-semibold">Downloader</h2>
  <p class="mb-3 text-sm text-slate-600">
    The Usenet downloader auto-pulled NZBs are submitted to. One downloader at a time.
  </p>

  {#if error}
    <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
  {/if}

  <form
    class="space-y-3"
    onsubmit={(e) => {
      e.preventDefault();
      void handleSave();
    }}
  >
    <fieldset class="flex gap-4">
      <legend class="mb-1 text-xs font-medium text-slate-600">Type</legend>
      <label class="flex items-center gap-1.5 text-sm text-slate-700">
        <input type="radio" value="sab" bind:group={form.kind} disabled={busy} />
        SABnzbd
      </label>
      <label class="flex items-center gap-1.5 text-sm text-slate-700">
        <input type="radio" value="nzbget" bind:group={form.kind} disabled={busy} />
        NZBGet
      </label>
    </fieldset>

    <label class="block text-xs font-medium text-slate-600">
      Base URL
      <input
        type="url"
        bind:value={form.base_url}
        disabled={busy}
        placeholder={form.kind === 'sab' ? 'http://localhost:8080' : 'http://localhost:6789'}
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>

    {#if form.kind === 'nzbget'}
      <label class="block text-xs font-medium text-slate-600">
        Username
        <input
          type="text"
          bind:value={form.username}
          disabled={busy}
          class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        />
      </label>
    {/if}

    <label class="block text-xs font-medium text-slate-600">
      {form.kind === 'sab' ? 'API key' : 'Password'}
      <input
        type="password"
        bind:value={form.secret}
        disabled={busy}
        placeholder={hasStoredSecret ? 'Leave blank to keep current' : 'Required'}
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>

    <label class="block text-xs font-medium text-slate-600">
      Category
      <input
        type="text"
        bind:value={form.category}
        disabled={busy}
        placeholder="(downloader default)"
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>

    <label class="flex items-center gap-2 text-sm text-slate-700">
      <input type="checkbox" bind:checked={form.enabled} disabled={busy} />
      Enabled
    </label>

    {#if testResult}
      <p class="text-xs {testResult.ok ? 'text-emerald-700' : 'text-red-700'}">
        {testResult.ok ? '✓ ' : '✗ '}{testResult.message}
      </p>
    {/if}

    <div class="flex gap-2">
      <Button type="submit" disabled={busy || form.base_url.trim() === ''}>Save</Button>
      <Button
        type="button"
        variant="secondary"
        onclick={handleTest}
        disabled={busy || form.base_url.trim() === ''}
      >
        Test connection
      </Button>
      <Button
        type="button"
        variant="ghost"
        onclick={handleClear}
        disabled={busy || current === null}
      >
        Clear
      </Button>
    </div>
  </form>
</section>
