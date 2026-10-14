<script lang="ts">
  // Small at-a-glance chip showing ComicVine's two lanes: interactive
  // (your searches and adds) and background (enrichment, credits,
  // catalog refresh), each as calls used / hourly budget, plus whether
  // background work is yielding to you or both lanes are paused on a
  // 429. Polls /api/cv/rate-limit every 15s (no DB behind it) so the
  // yield/pause state isn't badly stale. A lane's window rolls over an
  // hour after its first call, so a count is "since the window opened".
  import { onMount } from 'svelte';
  import { Cloud } from 'lucide-svelte';

  interface LaneCount {
    count: number;
    limit_per_hour: number;
    window_started_at_unix: number;
  }

  interface RateLimit {
    interactive: LaneCount;
    background: LaneCount;
    yielding_seconds: number;
    paused_seconds: number;
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
    timer = setInterval(() => void refresh(), 15_000);
    return () => {
      if (timer !== null) clearInterval(timer);
    };
  });

  const used = (l: LaneCount): number => (l.limit_per_hour > 0 ? l.count / l.limit_per_hour : 0);

  // Tone reflects the interactive lane's headroom (the one you wait
  // on) — green under 60%, amber 60-90%, red above 90% or while paused.
  const tone = $derived.by(() => {
    if (!state) return 'bg-slate-100 text-slate-600 border-slate-200';
    const pct = used(state.interactive);
    if (state.paused_seconds > 0 || pct >= 0.9) return 'bg-red-50 text-red-700 border-red-200';
    if (pct >= 0.6) return 'bg-amber-50 text-amber-800 border-amber-200';
    return 'bg-emerald-50 text-emerald-700 border-emerald-200';
  });
</script>

{#if state}
  <span
    class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {tone}"
    title={`ComicVine API, calls this hour. Interactive (your searches and adds): ${state.interactive.count} of ${state.interactive.limit_per_hour}. Background (enrichment, credits, refresh): ${state.background.count} of ${state.background.limit_per_hour}.`}
    data-testid="cv-rate-limit"
  >
    <Cloud class="size-3.5" aria-hidden="true" />
    CV I {state.interactive.count}/{state.interactive.limit_per_hour} · B {state.background.count}/{state.background.limit_per_hour}
    {#if state.paused_seconds > 0}
      · paused {state.paused_seconds}s
    {:else if state.yielding_seconds > 0}
      · yielding
    {/if}
  </span>
{/if}
