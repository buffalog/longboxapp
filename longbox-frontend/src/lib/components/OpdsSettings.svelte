<script lang="ts">
  // OPDS catalog access — a single-row config, so one form rather than a
  // list. Self-contained: seeded from the page's load data, then
  // maintained from each mutation's response. Mirrors DownloaderSettings.
  import { ApiError } from '$lib/api/client';
  import { regenerateOpdsToken, saveOpds, type Opds, type OpdsInput } from '$lib/api/opds';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from './Button.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  let { opds }: { opds: Opds } = $props();

  interface Form {
    enabled: boolean;
    username: string;
    password: string;
  }

  function toForm(o: Opds): Form {
    return { enabled: o.enabled, username: o.username, password: '' };
  }

  let current = $state<Opds>(opds);
  let form = $state<Form>(toForm(opds));
  let busy = $state(false);
  let error = $state<ApiError | null>(null);

  const hasStoredPassword = $derived(current.has_password);
  // Enabling the catalog without a username makes every /opds request 503;
  // surface that inline rather than letting the user wonder why it's dead.
  const enabledButUnconfigured = $derived(
    form.enabled && form.username.trim() === '' && !hasStoredPassword
  );

  function payload(): OpdsInput {
    return { enabled: form.enabled, username: form.username, password: form.password };
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
      current = await saveOpds(payload());
      form = toForm(current);
      toast.success('OPDS settings saved.');
    });
  }

  function handleRegenerate(): Promise<void> {
    return run(async () => {
      current = await regenerateOpdsToken();
      toast.success('New API token generated.');
    });
  }

  async function copy(value: string, label: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(`${label} copied.`);
    } catch {
      toast.warning('Could not copy to clipboard.');
    }
  }
</script>

<section class="rounded-lg border border-slate-200 bg-white p-4">
  <h2 class="mb-2 text-base font-semibold">OPDS</h2>
  <p class="mb-3 text-sm text-slate-600">
    Serve your library to OPDS comic readers (Chunky, Panels, KyBook). Reader apps connect with
    the username and password below, or with the API token.
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
    <label class="flex items-center gap-2 text-sm text-slate-700">
      <input type="checkbox" bind:checked={form.enabled} disabled={busy} />
      Enabled
    </label>

    {#if enabledButUnconfigured}
      <p class="text-xs text-amber-700">
        Set a username and password below — the catalog returns 503 until it's configured.
      </p>
    {/if}

    <label class="block text-xs font-medium text-slate-600">
      Username
      <input
        type="text"
        autocomplete="off"
        bind:value={form.username}
        disabled={busy}
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>

    <label class="block text-xs font-medium text-slate-600">
      Password
      <input
        type="password"
        autocomplete="new-password"
        bind:value={form.password}
        disabled={busy}
        placeholder={hasStoredPassword ? 'Leave blank to keep current' : 'Required'}
        class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
      />
    </label>

    <div class="flex gap-2">
      <Button type="submit" disabled={busy}>Save</Button>
    </div>
  </form>

  <hr class="my-4 border-slate-100" />

  <div class="space-y-3">
    <div>
      <span class="block text-xs font-medium text-slate-600">API token</span>
      <div class="mt-0.5 flex gap-2">
        <input
          type="text"
          readonly
          value={current.api_token}
          placeholder="Generated on first save"
          class="w-full rounded-md border border-slate-200 bg-slate-50 px-2 py-1 font-mono text-xs text-slate-700 focus:outline-none"
        />
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onclick={() => copy(current.api_token, 'API token')}
          disabled={current.api_token === ''}
        >
          Copy
        </Button>
        <Button type="button" variant="ghost" size="sm" onclick={handleRegenerate} disabled={busy}>
          Regenerate
        </Button>
      </div>
    </div>

    <div>
      <span class="block text-xs font-medium text-slate-600">Catalog URL</span>
      <div class="mt-0.5 flex gap-2">
        <input
          type="text"
          readonly
          value={current.catalog_url}
          class="w-full rounded-md border border-slate-200 bg-slate-50 px-2 py-1 font-mono text-xs text-slate-700 focus:outline-none"
        />
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onclick={() => copy(current.catalog_url, 'Catalog URL')}
        >
          Copy
        </Button>
      </div>
      <p class="mt-1 text-xs text-slate-500">Add this URL in your OPDS reader.</p>
    </div>
  </div>
</section>
