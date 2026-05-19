<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import NavBar from '$lib/components/NavBar.svelte';
  import ErrorBanner from '$lib/components/ErrorBanner.svelte';
  import Toast from '$lib/components/Toast.svelte';
  import { scanStatus } from '$lib/stores/scanStatus.svelte';

  let { children } = $props();

  onMount(() => {
    return scanStatus.subscribe();
  });
</script>

<NavBar />
<main class="mx-auto max-w-6xl px-4 py-6">
  {#if scanStatus.error}
    <div class="mb-4">
      <ErrorBanner error={scanStatus.error} onDismiss={() => (scanStatus.error = null)} />
    </div>
  {/if}
  {@render children?.()}
</main>

<!-- App-global toast stack. Fixed-positioned, so DOM placement is
     cosmetic; lives at layout root as app-global chrome. -->
<Toast />
