<script lang="ts">
  import { formatConfidence, formatMatchMethod } from '$lib/format';

  interface Props {
    confidence: number;
    method?: string;
  }

  let { confidence, method }: Props = $props();

  const tier = $derived.by(() => {
    if (confidence >= 0.85) return 'high';
    if (confidence >= 0.65) return 'mid';
    return 'low';
  });

  const colorClass = $derived.by(() => {
    switch (tier) {
      case 'high':
        return 'bg-status-owned';
      case 'mid':
        return 'bg-status-needs_review';
      case 'low':
        return 'bg-status-unmatched';
    }
  });

  const pct = $derived(`${Math.round(Math.max(0, Math.min(1, confidence)) * 100)}%`);
</script>

<div class="inline-flex items-center gap-2">
  <div
    class="h-1.5 w-16 overflow-hidden rounded-full bg-slate-200"
    role="meter"
    aria-valuemin="0"
    aria-valuemax="100"
    aria-valuenow={Math.round(confidence * 100)}
    aria-label="Match confidence"
  >
    <div class="h-full {colorClass}" style:width={pct}></div>
  </div>
  <span class="font-mono text-xs tabular-nums text-slate-700">{formatConfidence(confidence)}</span>
  {#if method}
    <span class="text-xs text-slate-500">{formatMatchMethod(method)}</span>
  {/if}
</div>
