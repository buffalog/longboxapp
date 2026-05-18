<script lang="ts">
  // Phase A settings UI is read-only. Editing requires restarting the
  // backend binary with new env vars; values shown here are reflected
  // from /api/settings.

  let { data } = $props();
  const s = $derived(data.settings);

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

      <dt class="font-medium text-slate-700">Version</dt>
      <dd>
        <span class="font-mono text-slate-900">{s.version}</span>
      </dd>
    </dl>
  </section>

  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">About</h2>
    <p class="text-sm text-slate-600">
      LongBox is a self-hosted comic library catalog. Phase A is the foundation: tracks watched
      series, matches files on disk against issues, surfaces what's owned and what's missing. No
      downloading or file mutation.
    </p>
  </section>
</div>
