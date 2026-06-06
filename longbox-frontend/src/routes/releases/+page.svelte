<script lang="ts">
  // The `Releases` nav item lands here; it has no UI of its own.
  // adapter-static can't honor a server-side `redirect()` from
  // `+page.ts` because there IS no server at runtime — the SPA shell
  // gets served verbatim and the SvelteKit client router takes over
  // here. So redirect client-side on mount.
  //
  // `replaceState: true` keeps the user's Back button working: the
  // browser history records `/releases/calendar` directly instead of
  // bouncing through the empty `/releases` entry.
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';

  onMount(() => {
    void goto('/releases/calendar', { replaceState: true });
  });
</script>

<p class="text-sm text-slate-500">Loading…</p>
