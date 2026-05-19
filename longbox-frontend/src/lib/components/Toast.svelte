<script lang="ts">
  import { onMount } from 'svelte';
  import { fly } from 'svelte/transition';
  import { CheckCircle2, XCircle, AlertTriangle, Info, X } from 'lucide-svelte';
  import { toast, type ToastType } from '$lib/stores/toast.svelte';

  // Per-type presentation. Colors are border + tint + icon; icons come
  // from lucide to match the rest of the app's iconography.
  const STYLES: Record<
    ToastType,
    { Icon: typeof CheckCircle2; ring: string; iconColor: string }
  > = {
    success: { Icon: CheckCircle2, ring: 'border-emerald-200 bg-emerald-50', iconColor: 'text-emerald-600' },
    error: { Icon: XCircle, ring: 'border-red-200 bg-red-50', iconColor: 'text-red-600' },
    warning: { Icon: AlertTriangle, ring: 'border-amber-200 bg-amber-50', iconColor: 'text-amber-600' },
    info: { Icon: Info, ring: 'border-blue-200 bg-blue-50', iconColor: 'text-blue-600' }
  };

  // Reduced-motion: under prefers-reduced-motion the slide-in collapses
  // to a zero-duration no-op and the exit fade is skipped (CSS handles
  // that via the motion-reduce variant below).
  let prefersReducedMotion = $state(false);
  onMount(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    prefersReducedMotion = mq.matches;
    const handler = (e: MediaQueryListEvent) => {
      prefersReducedMotion = e.matches;
    };
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  });

  function flyParams() {
    return prefersReducedMotion ? { x: 0, duration: 0 } : { x: 320, duration: 200 };
  }
</script>

<!-- Fixed bottom-right stack. aria-live=polite so screen readers
     announce toast text without interrupting. z-[60] sits above the
     sticky nav (z-40) and the scrubber (z-30) but below modals
     (z-50)... note: modals at z-50 would be UNDER this. Toasts should
     sit above modals so a toast fired from within a modal flow is
     visible — z-[60] is intentional. -->
<div
  class="pointer-events-none fixed bottom-4 right-4 z-[60] flex w-72 flex-col gap-2"
  aria-live="polite"
  aria-atomic="false"
>
  {#each toast.toasts as t (t.id)}
    {@const style = STYLES[t.type]}
    <div
      in:fly={flyParams()}
      class="pointer-events-auto flex items-start gap-2 rounded-lg border p-3 shadow-md transition-opacity duration-150 {style.ring}"
      class:opacity-0={t.exiting}
      class:motion-reduce:transition-none={true}
      role={t.type === 'error' || t.type === 'warning' ? 'alert' : 'status'}
    >
      <style.Icon class="mt-0.5 size-4 flex-shrink-0 {style.iconColor}" aria-hidden="true" />
      <div class="min-w-0 flex-1 text-sm text-slate-800">
        <p class="break-words">{t.message}</p>
        {#if t.action}
          <button
            type="button"
            class="mt-1 text-xs font-medium text-blue-600 hover:underline focus:outline-none focus:ring-2 focus:ring-blue-500"
            onclick={() => {
              t.action?.onClick();
              toast.dismiss(t.id);
            }}
          >{t.action.label}</button>
        {/if}
      </div>
      <button
        type="button"
        class="flex-shrink-0 rounded p-0.5 text-slate-400 hover:text-slate-700 focus:outline-none focus:ring-2 focus:ring-blue-500"
        aria-label="Dismiss notification"
        onclick={() => toast.dismiss(t.id)}
      >
        <X class="size-3.5" aria-hidden="true" />
      </button>
    </div>
  {/each}
</div>
