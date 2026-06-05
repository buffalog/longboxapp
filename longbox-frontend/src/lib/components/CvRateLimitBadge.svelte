<script lang="ts">
  // Small at-a-glance chip showing the ComicVine client's per-hour
  // call count. Polls /api/cv/rate-limit at 60s while idle (a
  // bursting enrichment / scan moves the counter fast but not so
  // fast that anything tighter helps UX). The sliding window the
  // backend uses isn't a strict 60-minute trailing window — it
  // rolls over when an hour has passed since the first call —
  // so the count is "since the window opened," displayed as "42/100"
  // with the configured cap as the denominator.
  import { onMount } from 'svelte';
  import { Cloud } from 'lucide-svelte';

  interface RateLimit {
    count: number;
    limit_per_hour: number;
    window_started_at_unix: number;
  }

  let state = $state<RateLimit | null>(null);
  let timer: ReturnType<typeof setInterval> | null = null;

  async function refresh(): Promise<void> {
    try {
      const r = await fetch('/api/cv/rate-limit');
      if (!r.ok) return;
      state = (await r.json()) as RateLimit;
    } catch {
      // Silent — the chip is a soft surface, a failed poll shouldn't
      // produce an error toast or a banner.
    }
  }

  onMount(() => {
    void refresh();
    timer = setInterval(() => void refresh(), 60_000);
    return () => {
      if (timer !== null) clearInterval(timer);
    };
  });

  // Tone reflects budget headroom — green under 60%, amber 60-90%,
  // red above 90%. Helps the user spot pre-throttle pressure at a
  // glance without staring at the absolute number.
  const tone = $derived.by(() => {
    if (!state || state.limit_per_hour === 0) {
      return 'bg-slate-100 text-slate-600 border-slate-200';
    }
    const pct = state.count / state.limit_per_hour;
    if (pct >= 0.9) return 'bg-red-50 text-red-700 border-red-200';
    if (pct >= 0.6) return 'bg-amber-50 text-amber-800 border-amber-200';
    return 'bg-emerald-50 text-emerald-700 border-emerald-200';
  });
</script>

{#if state}
  <span
    class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {tone}"
    title={`ComicVine API: ${state.count} of ${state.limit_per_hour} calls used in the current rolling hour window.`}
  >
    <Cloud class="size-3.5" aria-hidden="true" />
    CV {state.count}/{state.limit_per_hour}
  </span>
{/if}
