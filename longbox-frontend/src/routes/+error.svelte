<script lang="ts">
  // Root-level error boundary. SvelteKit walks UP the routing tree to
  // find the nearest +error.svelte for any unhandled load/render
  // throw, so a single file at src/routes/+error.svelte catches every
  // crash that escapes a deeper boundary. Without this the user sees
  // SvelteKit's bare framework error page (mostly white space on
  // production builds) — the each_key_duplicate crash on /missing was
  // the prototype for "blank screen, no recovery path."
  //
  // The page() store carries status + the error object (whose `message`
  // SvelteKit's `load`-error path includes; a plain rendering throw
  // surfaces a generic message). status is 0 for client-side render
  // errors and a real HTTP status for load failures.
  import { page } from '$app/stores';
  import { AlertTriangle } from 'lucide-svelte';
  import Button from '$lib/components/Button.svelte';

  function goBack(): void {
    if (typeof window !== 'undefined' && window.history.length > 1) {
      window.history.back();
    } else {
      // Hard fallback: history is empty (first navigation in this
      // tab landed on the error page). Send the user to the dashboard.
      window.location.href = '/';
    }
  }
</script>

<svelte:head>
  <title>Something went wrong · LongBox</title>
</svelte:head>

<div class="mx-auto flex max-w-xl flex-col items-center gap-4 py-12 text-center">
  <div class="rounded-full bg-amber-100 p-3 text-amber-700">
    <AlertTriangle class="size-8" aria-hidden="true" />
  </div>
  <h1 class="text-2xl font-bold">Something went wrong</h1>
  <p class="text-sm text-slate-600">
    {#if $page.status && $page.status !== 0}
      The page returned a <code class="font-mono">{$page.status}</code> error.
    {:else}
      The page hit an unexpected error.
    {/if}
  </p>
  {#if $page.error?.message}
    <pre
      class="w-full overflow-x-auto whitespace-pre-wrap rounded-md border border-slate-200 bg-slate-50 p-3 text-left font-mono text-xs text-slate-700">{$page.error.message}</pre>
  {/if}
  <div class="flex gap-2">
    <Button variant="secondary" onclick={goBack}>Go back</Button>
    <a href="/" class="inline-block">
      <Button variant="ghost">Dashboard</Button>
    </a>
  </div>
</div>
