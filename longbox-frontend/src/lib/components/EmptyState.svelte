<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Icon as LucideIcon } from 'lucide-svelte';

  interface Props {
    icon?: typeof LucideIcon;
    title: string;
    message?: string;
    cta?: Snippet;
  }

  let { icon: Icon, title, message, cta }: Props = $props();
</script>

<div class="flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed border-slate-300 bg-white p-10 text-center">
  {#if Icon}
    <Icon class="size-10 text-slate-400" aria-hidden="true" />
  {/if}
  <h2 class="text-lg font-semibold text-slate-700">{title}</h2>
  {#if message}
    <p class="max-w-sm text-sm text-slate-500">{message}</p>
  {/if}
  {#if cta}
    <div class="mt-2">{@render cta()}</div>
  {/if}
</div>
