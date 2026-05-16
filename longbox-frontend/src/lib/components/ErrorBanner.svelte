<script lang="ts">
  import { X } from 'lucide-svelte';
  import { ApiError } from '$lib/api/client';

  interface Props {
    error: ApiError | string | null;
    onDismiss?: () => void;
  }

  let { error, onDismiss }: Props = $props();

  const display = $derived.by(() => {
    if (!error) return null;
    if (error instanceof ApiError) {
      return { code: error.code, message: error.message, status: error.status };
    }
    return { code: 'error', message: error, status: null };
  });
</script>

{#if display}
  <div
    role="alert"
    class="flex items-start gap-3 rounded-md border border-red-300 bg-red-50 p-3 text-red-900"
  >
    <div class="flex-1">
      <div class="font-semibold">{display.message}</div>
      <div class="text-xs opacity-75">
        {display.code}{display.status ? ` · HTTP ${display.status}` : ''}
      </div>
    </div>
    {#if onDismiss}
      <button
        type="button"
        class="rounded p-1 hover:bg-red-100"
        aria-label="Dismiss error"
        onclick={onDismiss}
      >
        <X class="size-4" aria-hidden="true" />
      </button>
    {/if}
  </div>
{/if}
