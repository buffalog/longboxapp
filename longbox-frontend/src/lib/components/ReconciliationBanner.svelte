<script lang="ts">
  // Dashboard nudge: surfaces non-zero Library Tidy state and links to
  // /library/tidy. Self-contained — owns its own dismissal.
  import { ArrowRight, X } from 'lucide-svelte';

  interface Props {
    transitionCount: number;
    untrackedCount: number;
  }

  let { transitionCount, untrackedCount }: Props = $props();

  const STORAGE_KEY = 'longbox.reconcileBannerDismissed';

  // The banner's relevance is fully described by the two counts, so the
  // dismissal is a count *signature*: dismissing stores `t:u`, and the
  // banner re-appears only when a count changes. Dismiss silences the
  // nudge without hiding a situation that later worsens — and it never
  // touches the underlying reconciliation state.
  const signature = $derived(`${transitionCount}:${untrackedCount}`);

  function readDismissed(): string | null {
    try {
      return localStorage.getItem(STORAGE_KEY);
    } catch {
      // localStorage can throw in privacy modes — treat as not dismissed.
      return null;
    }
  }

  let dismissedSignature = $state<string | null>(readDismissed());

  const hasWork = $derived(transitionCount > 0 || untrackedCount > 0);
  const visible = $derived(hasWork && dismissedSignature !== signature);

  // One sentence per non-zero count, singular/plural-aware. The banner
  // can fire on either count alone, so a zero count is simply omitted.
  const message = $derived(
    [
      transitionCount > 0
        ? transitionCount === 1
          ? '1 series lost its files.'
          : `${transitionCount} series lost their files.`
        : null,
      untrackedCount > 0
        ? untrackedCount === 1
          ? '1 untracked folder detected.'
          : `${untrackedCount} untracked folders detected.`
        : null
    ]
      .filter((s): s is string => s !== null)
      .join(' ')
  );

  function dismiss(): void {
    dismissedSignature = signature;
    try {
      localStorage.setItem(STORAGE_KEY, signature);
    } catch {
      // localStorage unavailable — the in-memory `dismissedSignature`
      // still hides the banner for this page view.
    }
  }
</script>

{#if visible}
  <div
    class="mb-4 flex items-start justify-between gap-3 rounded-lg border border-amber-200 bg-amber-50 p-3"
    role="status"
  >
    <p class="text-sm text-amber-900">
      <span>{message}</span>
      <a
        href="/library/tidy"
        class="ml-1 inline-flex items-center gap-0.5 font-medium underline hover:no-underline"
      >
        Review<ArrowRight class="size-3.5" aria-hidden="true" />
      </a>
    </p>
    <button
      type="button"
      onclick={dismiss}
      aria-label="Dismiss"
      class="rounded p-1 text-amber-700 hover:bg-amber-100"
    >
      <X class="size-4" aria-hidden="true" />
    </button>
  </div>
{/if}
