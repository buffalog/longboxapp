<script lang="ts">
  import { Activity, Check, Clock } from 'lucide-svelte';
  import { scanStatus } from '$lib/stores/scanStatus.svelte';

  const label = $derived(() => {
    if (scanStatus.current) return 'Scanning…';
    if (scanStatus.recent.length > 0) return 'Idle';
    return 'No scans yet';
  });

  const Icon = $derived.by(() => {
    if (scanStatus.current) return Activity;
    if (scanStatus.recent.length > 0) return Check;
    return Clock;
  });

  const tone = $derived.by(() => {
    if (scanStatus.current) return 'bg-blue-100 text-blue-700 border-blue-200';
    if (scanStatus.recent.length > 0) return 'bg-green-50 text-green-700 border-green-200';
    return 'bg-slate-100 text-slate-600 border-slate-200';
  });
</script>

<a
  href="/scans"
  class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {tone}"
  aria-label={`Scan status: ${label()}`}
>
  <Icon class="size-3.5 {scanStatus.current ? 'animate-pulse' : ''}" aria-hidden="true" />
  {label()}
</a>
