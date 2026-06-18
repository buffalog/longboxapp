<script lang="ts">
  import type { Snippet } from 'svelte';
  import { X } from 'lucide-svelte';

  interface Props {
    open: boolean;
    title: string;
    onClose: () => void;
    children: Snippet;
    /** Tailwind max-width class for the dialog. Defaults to a narrow
     *  form width; wider surfaces (e.g. a results table) pass `max-w-4xl`. */
    maxWidth?: string;
  }

  let { open, title, onClose, children, maxWidth = 'max-w-lg' }: Props = $props();

  function handleKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') onClose();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-4"
    role="dialog"
    aria-modal="true"
    aria-labelledby="modal-title"
  >
    <button
      type="button"
      class="absolute inset-0 cursor-default"
      aria-label="Close dialog"
      onclick={onClose}
    ></button>
    <div class="relative w-full {maxWidth} rounded-lg bg-white shadow-xl">
      <header class="flex items-center justify-between border-b border-slate-200 px-4 py-3">
        <h2 id="modal-title" class="text-base font-semibold">{title}</h2>
        <button
          type="button"
          class="rounded p-1 hover:bg-slate-100"
          aria-label="Close"
          onclick={onClose}
        >
          <X class="size-4" aria-hidden="true" />
        </button>
      </header>
      <div class="p-4">{@render children()}</div>
    </div>
  </div>
{/if}
