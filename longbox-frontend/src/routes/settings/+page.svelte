<script lang="ts">
  // Phase A settings UI is read-only for env vars. Editing requires
  // restarting the backend binary; values shown here are reflected from
  // /api/settings. Publisher filters are the one editable surface.
  import { invalidateAll } from '$app/navigation';
  import { Trash2, RotateCcw, Power, FolderSync } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';
  import { restartServer } from '$lib/api/admin';
  import { triggerPostprocess } from '$lib/api/postprocess';
  import { addFilter, deleteFilter, resetFiltersToDefaults } from '$lib/api/publishers';
  import { updateSetting, type EditableSettingKey } from '$lib/api/settings';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from '$lib/components/Button.svelte';
  import DownloaderSettings from '$lib/components/DownloaderSettings.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import IndexerSettings from '$lib/components/IndexerSettings.svelte';
  import OpdsSettings from '$lib/components/OpdsSettings.svelte';
  import WebhookSettings from '$lib/components/WebhookSettings.svelte';

  let { data } = $props();
  const s = $derived(data.settings);

  let newPublisher = $state('');
  let busy = $state(false);
  let error = $state<ApiError | null>(null);

  // Editable settings — one draft + saving flag per tunable. Drafts
  // are seeded from the server's canonical value on mount; we don't
  // auto-resync them after server changes (would clobber an in-flight
  // edit). After a successful save we update both the local "saved"
  // mirror and re-invalidate so other surfaces of `data.settings`
  // reflect the new value.
  let matchThresholdDraft = $state(String(data.settings.match_confidence_threshold));
  let matchThresholdSaving = $state(false);
  let pullThresholdDraft = $state(String(data.settings.pull_indexer_match_threshold));
  let pullThresholdSaving = $state(false);
  let enrichKnownDraft = $state(
    String(data.settings.cv_enrichment_title_threshold_year_known)
  );
  let enrichKnownSaving = $state(false);
  let enrichUnknownDraft = $state(
    String(data.settings.cv_enrichment_title_threshold_year_unknown)
  );
  let enrichUnknownSaving = $state(false);
  let pullKeywordsDraft = $state(data.settings.pull_exclusion_keywords);
  let pullKeywordsSaving = $state(false);
  let hostLibraryPathDraft = $state(data.settings.host_library_path);
  let hostLibraryPathSaving = $state(false);
  let minFileSizeMbDraft = $state(String(data.settings.min_file_size_mb));
  let minFileSizeMbSaving = $state(false);

  /// Client-side megabyte validation. Mirrors the backend's
  /// `parse_megabytes`: whole number 0..=10240. The `0` sentinel
  /// disables the size floor entirely (Phase B accepts everything).
  function validateMegabytes(raw: string): string | null {
    const trimmed = raw.trim();
    if (trimmed === '') return 'Enter a whole number of MB (use 0 to disable the floor).';
    if (!/^\d+$/.test(trimmed))
      return `'${raw}' must be a whole number — fractional MB aren't accepted.`;
    const n = Number(trimmed);
    if (!Number.isFinite(n)) return `'${raw}' is not a valid number.`;
    if (n > 10240) return `Must be ≤ 10240 MB (10 GB); got ${n}.`;
    return null;
  }

  /// Client-side threshold validation. Mirrors the backend's
  /// `parse_threshold`: must be a finite number in [0.0, 1.0].
  /// Returns a user-friendly error message on failure, or null on
  /// success. We pre-check before hitting the wire so a malformed
  /// value gets caught instantly without a round-trip.
  function validateThreshold(raw: string): string | null {
    const trimmed = raw.trim();
    if (trimmed === '') return 'Enter a number between 0.0 and 1.0.';
    const n = Number(trimmed);
    if (!Number.isFinite(n)) return `'${raw}' is not a valid number.`;
    if (n < 0 || n > 1) return `Must be between 0.0 and 1.0 (got ${n}).`;
    return null;
  }

  async function saveTunable(
    key: EditableSettingKey,
    draft: string,
    setSaving: (b: boolean) => void,
    validator: ((raw: string) => string | null) | null,
    successMessage: string
  ): Promise<void> {
    if (validator) {
      const errMsg = validator(draft);
      if (errMsg) {
        toast.warning(errMsg);
        return;
      }
    }
    setSaving(true);
    try {
      await updateSetting(key, draft);
      toast.success(successMessage);
      await invalidateAll();
    } catch (e) {
      const message = e instanceof ApiError ? e.message : 'Could not save the setting.';
      toast.warning(message);
    } finally {
      setSaving(false);
    }
  }

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
    // `MATCH_THRESHOLD` env var was the original boot-time tunable.
    // It's now superseded by the `match_confidence_threshold` DB row
    // exposed in the Tunable settings section below — that's the
    // value the scanner / Phase B actually read. Displaying both
    // here just produced "0.85 set via MATCH_THRESHOLD" right above
    // "Currently saved: 0.75" on the same page. The env value is now
    // documented as a seed-only fallback in the backend and elided
    // from the read-only Configuration list.
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

  // Manual Phase B sweep. Single in-flight at a time — multiple
  // sweeps would just walk the same files; the underlying processor
  // is idempotent but the UX is noisier than useful.
  let processing = $state(false);

  async function handleProcessDownloads(): Promise<void> {
    if (processing) return;
    processing = true;
    try {
      const summary = await triggerPostprocess();
      const total =
        summary.processed + summary.skipped + summary.conflicts + summary.failed;
      if (total === 0) {
        toast.success('Watch folder is empty — nothing to process.');
      } else {
        const parts: string[] = [];
        parts.push(`processed ${summary.processed}`);
        if (summary.skipped > 0) parts.push(`skipped ${summary.skipped}`);
        if (summary.conflicts > 0) parts.push(`conflicts ${summary.conflicts}`);
        if (summary.failed > 0) parts.push(`failed ${summary.failed}`);
        toast.success(`Phase B sweep complete · ${parts.join(' · ')}`);
      }
    } catch (e) {
      const message = e instanceof ApiError ? e.message : 'Could not run the sweep.';
      toast.warning(message);
    } finally {
      processing = false;
    }
  }
</script>

<h1 class="mb-4 text-2xl font-bold">Settings</h1>

<div class="space-y-4">
  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">Configuration</h2>
    <p class="mb-3 text-sm text-slate-600">
      LongBox reads these settings from environment variables at startup. They're not editable
      from the UI — restart the container with different env vars to change them.
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
          <span class="ml-2 text-xs text-emerald-700">(watching)</span>
        {:else}
          <span class="text-slate-500">— (not configured)</span>
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
    <h2 class="mb-2 text-base font-semibold">Tunable settings</h2>
    <p class="mb-4 text-sm text-slate-600">
      These settings are read live from the database by their consumers — the scanner per scan,
      the enrichment worker per cycle, and the pull engine per sweep. Changes take effect on the
      next run without a container restart. Initial defaults come from environment variables at
      first boot; subsequent edits override them.
    </p>

    <div class="space-y-5">
      <div class="border-t border-slate-100 pt-4 first:border-t-0 first:pt-0">
        <h3 class="text-sm font-semibold text-slate-800">Match confidence threshold</h3>
        <p class="mb-2 text-xs text-slate-500">
          Catalog match cutoff between <code>owned</code> and <code>needs_review</code>. Read live
          by BOTH the scanner (per scan) and Phase B (per watch-folder sweep) — both agree on
          what "confident enough to claim" means by reading this one row. Range 0.0–1.0. Currently
          saved: <code class="font-mono">{s.match_confidence_threshold}</code>.
        </p>
        <form
          class="flex gap-2"
          onsubmit={(e) => {
            e.preventDefault();
            void saveTunable(
              'match_confidence_threshold',
              matchThresholdDraft,
              (b) => (matchThresholdSaving = b),
              validateThreshold,
              'Match threshold saved.'
            );
          }}
        >
          <input
            type="text"
            inputmode="decimal"
            bind:value={matchThresholdDraft}
            disabled={matchThresholdSaving}
            class="w-32 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            aria-label="Match confidence threshold value"
          />
          <Button type="submit" loading={matchThresholdSaving} disabled={matchThresholdSaving}>
            Save
          </Button>
        </form>
      </div>

      <div class="border-t border-slate-100 pt-4">
        <h3 class="text-sm font-semibold text-slate-800">Pull indexer match threshold</h3>
        <p class="mb-2 text-xs text-slate-500">
          NZB-to-series similarity gate the pull engine applies BEFORE handing a release to
          SABnzbd. Pre-grab errors are asymmetrically costly (wasted bandwidth, churned SAB,
          catalog poison), so the default sits stricter than the catalog cutoff above. Read
          per-sweep. Range 0.0–1.0. Currently saved:
          <code class="font-mono">{s.pull_indexer_match_threshold}</code>.
        </p>
        <form
          class="flex gap-2"
          onsubmit={(e) => {
            e.preventDefault();
            void saveTunable(
              'pull_indexer_match_threshold',
              pullThresholdDraft,
              (b) => (pullThresholdSaving = b),
              validateThreshold,
              'Pull indexer threshold saved.'
            );
          }}
        >
          <input
            type="text"
            inputmode="decimal"
            bind:value={pullThresholdDraft}
            disabled={pullThresholdSaving}
            class="w-32 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            aria-label="Pull indexer match threshold value"
          />
          <Button type="submit" loading={pullThresholdSaving} disabled={pullThresholdSaving}>
            Save
          </Button>
        </form>
      </div>

      <div class="border-t border-slate-100 pt-4">
        <h3 class="text-sm font-semibold text-slate-800">
          Enrichment title threshold (year known)
        </h3>
        <p class="mb-2 text-xs text-slate-500">
          CV-enrichment title-similarity gate for candidates whose start year matches the
          catalog's. Range 0.0–1.0. Currently saved:
          <code class="font-mono">{s.cv_enrichment_title_threshold_year_known}</code>.
        </p>
        <form
          class="flex gap-2"
          onsubmit={(e) => {
            e.preventDefault();
            void saveTunable(
              'cv_enrichment_title_threshold_year_known',
              enrichKnownDraft,
              (b) => (enrichKnownSaving = b),
              validateThreshold,
              'Enrichment year-known threshold saved.'
            );
          }}
        >
          <input
            type="text"
            inputmode="decimal"
            bind:value={enrichKnownDraft}
            disabled={enrichKnownSaving}
            class="w-32 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            aria-label="Enrichment title threshold (year known) value"
          />
          <Button type="submit" loading={enrichKnownSaving} disabled={enrichKnownSaving}>
            Save
          </Button>
        </form>
      </div>

      <div class="border-t border-slate-100 pt-4">
        <h3 class="text-sm font-semibold text-slate-800">
          Enrichment title threshold (year unknown)
        </h3>
        <p class="mb-2 text-xs text-slate-500">
          Stricter title-similarity gate for candidates with no start year — the conservative
          default catches more false matches when one anchor is missing. Range 0.0–1.0. Currently
          saved: <code class="font-mono">{s.cv_enrichment_title_threshold_year_unknown}</code>.
        </p>
        <form
          class="flex gap-2"
          onsubmit={(e) => {
            e.preventDefault();
            void saveTunable(
              'cv_enrichment_title_threshold_year_unknown',
              enrichUnknownDraft,
              (b) => (enrichUnknownSaving = b),
              validateThreshold,
              'Enrichment year-unknown threshold saved.'
            );
          }}
        >
          <input
            type="text"
            inputmode="decimal"
            bind:value={enrichUnknownDraft}
            disabled={enrichUnknownSaving}
            class="w-32 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            aria-label="Enrichment title threshold (year unknown) value"
          />
          <Button type="submit" loading={enrichUnknownSaving} disabled={enrichUnknownSaving}>
            Save
          </Button>
        </form>
      </div>

      <div class="border-t border-slate-100 pt-4">
        <h3 class="text-sm font-semibold text-slate-800">Pull exclusion keywords</h3>
        <p class="mb-2 text-xs text-slate-500">
          Comma-separated list of release-title substrings the pull pre-grab filter drops —
          typically digital-only formats like "infinity comic". Case-insensitive substring match
          at the pull engine. Currently saved:
          {#if s.pull_exclusion_keywords}
            <code class="font-mono">{s.pull_exclusion_keywords}</code>
          {:else}
            <span class="text-slate-400">(empty — no exclusions)</span>
          {/if}.
        </p>
        <form
          class="flex gap-2"
          onsubmit={(e) => {
            e.preventDefault();
            void saveTunable(
              'pull_exclusion_keywords',
              pullKeywordsDraft,
              (b) => (pullKeywordsSaving = b),
              null,
              'Pull exclusion keywords saved.'
            );
          }}
        >
          <input
            type="text"
            bind:value={pullKeywordsDraft}
            disabled={pullKeywordsSaving}
            placeholder="e.g. infinity comic, infinite comic, digital"
            class="flex-1 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            aria-label="Pull exclusion keywords value"
          />
          <Button type="submit" loading={pullKeywordsSaving} disabled={pullKeywordsSaving}>
            Save
          </Button>
        </form>
      </div>

      <div class="border-t border-slate-100 pt-4">
        <h3 class="text-sm font-semibold text-slate-800">Host library path</h3>
        <p class="mb-2 text-xs text-slate-500">
          Host-side path your library mounts to. Used by "Show in Finder" on series pages — the
          container only sees <code class="font-mono">{s.library_root_path}</code>, so we
          substitute that prefix with this value to build a <code class="font-mono">file://</code>
          URL the host OS can open. Leave empty to disable Show-in-Finder (the button falls back
          to a clipboard copy). Currently saved:
          {#if s.host_library_path}
            <code class="font-mono">{s.host_library_path}</code>
          {:else}
            <span class="text-slate-400">(empty — Show in Finder will copy the container path)</span>
          {/if}.
        </p>
        <form
          class="flex gap-2"
          onsubmit={(e) => {
            e.preventDefault();
            void saveTunable(
              'host_library_path',
              hostLibraryPathDraft,
              (b) => (hostLibraryPathSaving = b),
              null,
              'Host library path saved.'
            );
          }}
        >
          <input
            type="text"
            bind:value={hostLibraryPathDraft}
            disabled={hostLibraryPathSaving}
            placeholder="e.g. /Volumes/Comics or /Users/you/Library/Comics"
            class="flex-1 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            aria-label="Host library path value"
          />
          <Button type="submit" loading={hostLibraryPathSaving} disabled={hostLibraryPathSaving}>
            Save
          </Button>
        </form>
      </div>

      <div class="border-t border-slate-100 pt-4">
        <h3 class="text-sm font-semibold text-slate-800">Minimum file size (MB)</h3>
        <p class="mb-2 text-xs text-slate-500">
          Phase B size floor. Any CBR/CBZ/CB7 arriving in the watch folder smaller than this gets
          rejected with a WARN log and stays in <code class="font-mono">/watch/</code> — catches
          partial/corrupt SAB deliveries (e.g. an Absolute Batman issue that should be ~50 MB
          arriving at 16 MB with five pages). Set to <code>0</code> to disable the floor entirely.
          Currently saved:
          <code class="font-mono">{s.min_file_size_mb} MB</code>.
        </p>
        <form
          class="flex gap-2"
          onsubmit={(e) => {
            e.preventDefault();
            void saveTunable(
              'min_file_size_mb',
              minFileSizeMbDraft,
              (b) => (minFileSizeMbSaving = b),
              validateMegabytes,
              'Minimum file size saved.'
            );
          }}
        >
          <input
            type="text"
            inputmode="numeric"
            bind:value={minFileSizeMbDraft}
            disabled={minFileSizeMbSaving}
            class="w-32 rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
            aria-label="Minimum file size MB value"
          />
          <Button type="submit" loading={minFileSizeMbSaving} disabled={minFileSizeMbSaving}>
            Save
          </Button>
        </form>
      </div>
    </div>
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

  <OpdsSettings opds={data.opds} users={data.opdsUsers} />

  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">About</h2>
    <p class="text-sm text-slate-600">
      LongBox is a self-hosted comic library catalog. It tracks watched series, matches files on
      disk against issues, surfaces what's owned and what's missing, and integrates with newznab
      indexers to fill the gaps.
    </p>
  </section>

  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">System</h2>
    <div class="mb-4">
      <p class="mb-3 text-sm text-slate-600">
        Drain the download watch folder now: parse every CBZ/CBR LongBox can find, match it
        against the catalog, and move owned files into their library folders. Unmatched
        files stay in the watch folder — there's no <code class="rounded bg-slate-100 px-1 py-0.5 text-xs">_unsorted/</code>
        bucket — so you can review or delete them directly. Use this after dropping files
        into the watch folder manually instead of waiting for the background watcher.
      </p>
      <Button
        variant="secondary"
        onclick={handleProcessDownloads}
        loading={processing}
        disabled={processing}
      >
        <FolderSync class="size-4" aria-hidden="true" />
        {processing ? 'Processing…' : 'Process downloads'}
      </Button>
    </div>
    <div>
      <p class="mb-3 text-sm text-slate-600">
        Restart LongBox to pick up new environment variables or recover from a stuck worker.
        The container exits and Docker brings it back up — in-flight requests will fail and
        any running scans or sweeps will be cut short.
      </p>
      <Button variant="warning" onclick={handleRestart} loading={restarting} disabled={restarting}>
        <Power class="size-4" aria-hidden="true" />
        {restarting ? 'Restarting…' : 'Restart LongBox'}
      </Button>
    </div>
  </section>
</div>
