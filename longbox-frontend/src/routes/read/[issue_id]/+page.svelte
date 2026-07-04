<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import {
    getPageCount,
    getIssue,
    getReadingProgress,
    saveReadingProgress,
    pageImageUrl
  } from '$lib/api/reader';

  let { data } = $props();
  const issueId = $derived(data.issueId);

  // --- Reader state --------------------------------------------------------
  let totalPages = $state(0);
  // Primary position. In spread mode this is the LEFT page of the current
  // spread; page 1 (cover) always shows alone.
  let leftPage = $state(1);
  let seriesId = $state<number | null>(null);
  let fitMode = $state<'width' | 'page'>('width');
  let viewMode = $state<'single' | 'spread'>('single');
  let hudVisible = $state(false);
  let loading = $state(true);
  let loadError = $state<string | null>(null);

  let hudTimer: ReturnType<typeof setTimeout> | null = null;
  // When the HUD was last revealed, so a middle *click* that arrives right
  // after the reveal-on-hover move doesn't immediately toggle it back off.
  let hudShownAt = 0;
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  const clamp = (n: number, lo: number, hi: number) => Math.min(Math.max(n, lo), hi);

  // Whether the current spread shows two pages side by side. The cover
  // (leftPage 1) and a trailing lone page on an even-total book show alone.
  const showPair = $derived(
    viewMode === 'spread' && leftPage >= 2 && leftPage + 1 <= totalPages
  );

  // --- Load ----------------------------------------------------------------
  async function init() {
    try {
      const [count, issue, progress] = await Promise.all([
        getPageCount(issueId),
        getIssue(issueId),
        getReadingProgress(issueId)
      ]);
      totalPages = count.count;
      seriesId = issue.series_id;
      leftPage = clamp(progress.last_page, 1, Math.max(1, totalPages));
    } catch (e) {
      loadError = e instanceof Error ? e.message : 'Failed to open reader';
    } finally {
      loading = false;
    }
  }

  // --- Navigation ----------------------------------------------------------
  // Spread stepping: cover (1) pairs forward to 2, then advances two at a time,
  // clamped at N so the last spread never points past the final page.
  const nextSpread = (lp: number, n: number) => (lp === 1 ? 2 : Math.min(lp + 2, n));
  const prevSpread = (lp: number) => (lp <= 2 ? 1 : lp - 2);

  function next() {
    leftPage =
      viewMode === 'spread'
        ? clamp(nextSpread(leftPage, totalPages), 1, totalPages)
        : clamp(leftPage + 1, 1, totalPages);
  }
  function prev() {
    leftPage = viewMode === 'spread' ? prevSpread(leftPage) : clamp(leftPage - 1, 1, totalPages);
  }

  function toggleFit() {
    fitMode = fitMode === 'width' ? 'page' : 'width';
  }

  function toggleView() {
    viewMode = viewMode === 'single' ? 'spread' : 'single';
    localStorage.setItem('longbox_reader_spread', String(viewMode === 'spread'));
  }

  function toggleFullscreen() {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen?.();
    } else {
      document.exitFullscreen?.();
    }
  }

  function exit() {
    goto(seriesId ? `/series/${seriesId}` : '/series');
  }

  function onKeydown(e: KeyboardEvent) {
    switch (e.key) {
      case 'ArrowRight':
        next();
        break;
      case 'ArrowLeft':
        prev();
        break;
      case 'f':
      case 'F':
        toggleFullscreen();
        break;
      case 't':
      case 'T':
        toggleFit();
        break;
      case 's':
      case 'S':
        toggleView();
        break;
    }
  }

  // --- HUD -----------------------------------------------------------------
  function armHudTimer() {
    if (hudTimer) clearTimeout(hudTimer);
    hudTimer = setTimeout(() => (hudVisible = false), 2000);
  }
  function showHud() {
    if (!hudVisible) hudShownAt = performance.now();
    hudVisible = true;
    armHudTimer();
  }
  function toggleHud() {
    // Only hide if the HUD has been up for a moment — otherwise the
    // reveal-on-hover move that precedes a middle click would be undone by
    // the click itself, and the HUD could never be summoned by clicking.
    if (hudVisible && performance.now() - hudShownAt > 400) {
      hudVisible = false;
      if (hudTimer) clearTimeout(hudTimer);
    } else {
      showHud();
    }
  }
  function holdHud() {
    if (hudTimer) clearTimeout(hudTimer);
    hudVisible = true;
  }

  /** Which third of the viewport a click/move landed in. */
  function zoneOf(clientX: number): 'prev' | 'middle' | 'next' {
    const w = window.innerWidth;
    if (clientX < w / 3) return 'prev';
    if (clientX > (2 * w) / 3) return 'next';
    return 'middle';
  }

  function onClick(e: MouseEvent) {
    switch (zoneOf(e.clientX)) {
      case 'prev':
        prev();
        break;
      case 'next':
        next();
        break;
      case 'middle':
        toggleHud();
        break;
    }
  }

  function onMouseMove(e: MouseEvent) {
    if (zoneOf(e.clientX) === 'middle') showHud();
  }

  // --- Preloading + progress ----------------------------------------------
  /** Warm the browser cache for a page (fire-and-forget). No-op out of range. */
  function preload(page: number) {
    if (page >= 1 && page <= totalPages) {
      const img = new Image();
      img.src = pageImageUrl(issueId, page);
    }
  }

  /** Debounced, fire-and-forget progress write — coalesces rapid page turns. */
  function scheduleSave(page: number) {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      saveTimer = null;
      void saveReadingProgress(issueId, page).catch(() => {});
    }, 500);
  }

  // On every position change (once loaded): preload the next page(s) for the
  // active mode and schedule a debounced progress save.
  $effect(() => {
    const lp = leftPage;
    if (loading || totalPages === 0) return;
    if (viewMode === 'spread') {
      preload(lp + 2);
      preload(lp + 3);
    } else {
      preload(lp + 1);
      preload(lp + 2);
    }
    scheduleSave(lp);
  });

  onMount(() => {
    if (localStorage.getItem('longbox_reader_spread') === 'true') {
      viewMode = 'spread';
    }
    void init();
    window.addEventListener('keydown', onKeydown);
    return () => {
      window.removeEventListener('keydown', onKeydown);
      if (hudTimer) clearTimeout(hudTimer);
      // Flush a pending progress write so exiting right after a page turn
      // still persists the final position.
      if (saveTimer) {
        clearTimeout(saveTimer);
        void saveReadingProgress(issueId, leftPage).catch(() => {});
      }
    };
  });

  // HUD page label: a range when a two-page spread is showing, else a single.
  const pageLabel = $derived(
    showPair ? `Pages ${leftPage}–${leftPage + 1} of ${totalPages}` : `Page ${leftPage} of ${totalPages}`
  );
</script>

<svelte:head>
  <title>Reading — LongBox</title>
</svelte:head>

<!-- Full-screen overlay: escapes the app's NavBar + max-width layout by
     covering the whole viewport. Click zones and hover drive navigation;
     keyboard nav is handled by the document-level keydown listener, so the
     pointer-surface a11y lint is intentionally waived here. -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="fixed inset-0 z-50 flex justify-center bg-black text-white {viewMode === 'single' &&
  fitMode === 'width'
    ? 'items-start overflow-y-auto'
    : 'items-center overflow-hidden'}"
  onclick={onClick}
  onmousemove={onMouseMove}
>
  {#if loading}
    <div class="flex h-full items-center gap-2 text-sm text-slate-300" role="status">
      <span
        class="inline-block size-6 animate-spin rounded-full border-2 border-current border-r-transparent"
        aria-hidden="true"
      ></span>
      Loading…
    </div>
  {:else if loadError}
    <div class="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
      <p class="text-sm text-red-400">{loadError}</p>
      <button class="rounded border border-slate-500 px-3 py-1 text-sm" onclick={exit}>
        Back to series
      </button>
    </div>
  {:else if totalPages === 0}
    <div class="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
      <p class="text-sm text-slate-300">This issue has no readable pages.</p>
      <button class="rounded border border-slate-500 px-3 py-1 text-sm" onclick={exit}>
        Back to series
      </button>
    </div>
  {:else if viewMode === 'spread'}
    <!-- Two-up spread: full-height row, no gap. A lone page (cover, or the last
         page of an even-total book) is centered at half width. -->
    <div class="flex h-screen w-full items-center justify-center">
      {#if showPair}
        <img
          src={pageImageUrl(issueId, leftPage)}
          alt={`Page ${leftPage}`}
          class="h-screen w-1/2 select-none object-contain"
          draggable="false"
        />
        <img
          src={pageImageUrl(issueId, leftPage + 1)}
          alt={`Page ${leftPage + 1}`}
          class="h-screen w-1/2 select-none object-contain"
          draggable="false"
        />
      {:else}
        <img
          src={pageImageUrl(issueId, leftPage)}
          alt={`Page ${leftPage}`}
          class="mx-auto h-screen w-1/2 select-none object-contain"
          draggable="false"
        />
      {/if}
    </div>
  {:else}
    <img
      src={pageImageUrl(issueId, leftPage)}
      alt={`Page ${leftPage}`}
      class="select-none {fitMode === 'width' ? 'h-auto w-full' : 'max-h-screen w-auto'}"
      style={fitMode === 'page' ? 'height:100vh;width:auto;object-fit:contain' : ''}
      draggable="false"
    />
  {/if}

  <!-- Exit (top-left). stopPropagation so the click doesn't also page-turn. -->
  <button
    class="fixed left-3 top-3 z-[60] rounded-full bg-black/50 px-3 py-1.5 text-sm text-white
           opacity-70 transition hover:opacity-100"
    onclick={(e) => {
      e.stopPropagation();
      exit();
    }}
  >
    ✕ Exit
  </button>

  <!-- HUD (bottom). -->
  {#if hudVisible && !loading && !loadError}
    <div
      class="fixed inset-x-0 bottom-0 z-[60] flex items-center justify-center gap-4 bg-black/70
             px-4 py-3 text-sm"
      role="toolbar"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onmouseenter={holdHud}
      onmouseleave={armHudTimer}
    >
      <span class="tabular-nums text-slate-200">{pageLabel}</span>
      <button
        class="rounded border border-slate-500 px-2 py-0.5 text-slate-200 hover:bg-white/10"
        onclick={toggleView}
        title="Toggle single / two-page (S)"
      >
        {viewMode === 'spread' ? '2P' : '1P'}
      </button>
      {#if viewMode === 'single'}
        <button
          class="rounded border border-slate-500 px-2 py-0.5 text-slate-200 hover:bg-white/10"
          onclick={toggleFit}
          title="Toggle fit width / page (T)"
        >
          {fitMode === 'width' ? 'Fit width' : 'Fit page'}
        </button>
      {/if}
    </div>
  {/if}
</div>
