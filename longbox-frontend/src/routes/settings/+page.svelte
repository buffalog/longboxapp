<script lang="ts">
  // Phase A settings UI is read-only. Editing requires restarting the
  // backend binary with new env vars. The values shown here are baked in
  // at server start.
</script>

<h1 class="mb-4 text-2xl font-bold">Settings</h1>

<div class="space-y-4">
  <section class="rounded-lg border border-slate-200 bg-white p-4">
    <h2 class="mb-2 text-base font-semibold">Configuration</h2>
    <p class="mb-3 text-sm text-slate-600">
      LongBox reads its settings from environment variables at startup. The values are not editable
      from the UI in Phase A — restart the binary with different env vars to change them.
    </p>
    <dl class="grid grid-cols-1 gap-2 text-sm sm:grid-cols-[10rem_1fr]">
      <dt class="font-medium text-slate-700">Library root</dt>
      <dd class="font-mono text-slate-600">set via <code>LIBRARY_ROOT_PATH</code></dd>

      <dt class="font-medium text-slate-700">Database</dt>
      <dd class="font-mono text-slate-600">set via <code>DATABASE_URL</code></dd>

      <dt class="font-medium text-slate-700">ComicVine API</dt>
      <dd class="text-slate-600">configured via <code>COMICVINE_API_KEY</code> (value never displayed)</dd>

      <dt class="font-medium text-slate-700">Match threshold</dt>
      <dd class="text-slate-600">set via <code>MATCH_THRESHOLD</code> (default 0.85)</dd>

      <dt class="font-medium text-slate-700">Log level</dt>
      <dd class="text-slate-600">set via <code>LOG_LEVEL</code> (default info)</dd>

      <dt class="font-medium text-slate-700">Bind address</dt>
      <dd class="text-slate-600">set via <code>BIND_ADDR</code> (default 0.0.0.0:3000)</dd>
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
