<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLButtonAttributes } from 'svelte/elements';

  type Variant = 'primary' | 'secondary' | 'danger' | 'ghost';
  type Size = 'sm' | 'md' | 'lg';

  interface Props extends HTMLButtonAttributes {
    variant?: Variant;
    size?: Size;
    loading?: boolean;
    children: Snippet;
  }

  let {
    variant = 'primary',
    size = 'md',
    loading = false,
    disabled = false,
    children,
    type = 'button',
    class: extraClass = '',
    ...rest
  }: Props = $props();

  const sizeClass: Record<Size, string> = {
    sm: 'px-2.5 py-1 text-sm',
    md: 'px-3.5 py-1.5 text-sm',
    lg: 'px-4 py-2 text-base'
  };

  const variantClass: Record<Variant, string> = {
    primary:
      'bg-slate-900 text-white hover:bg-slate-800 disabled:bg-slate-400 disabled:cursor-not-allowed',
    secondary:
      'bg-white text-slate-900 border border-slate-300 hover:bg-slate-50 disabled:bg-slate-100 disabled:text-slate-400 disabled:cursor-not-allowed',
    danger:
      'bg-red-600 text-white hover:bg-red-500 disabled:bg-red-300 disabled:cursor-not-allowed',
    ghost:
      'bg-transparent text-slate-700 hover:bg-slate-100 disabled:text-slate-400 disabled:cursor-not-allowed'
  };
</script>

<button
  {type}
  disabled={disabled || loading}
  class="inline-flex items-center gap-1.5 rounded-md font-medium transition {sizeClass[size]} {variantClass[variant]} {extraClass}"
  {...rest}
>
  {#if loading}
    <span class="inline-block size-3 animate-spin rounded-full border-2 border-current border-r-transparent" aria-hidden="true"></span>
  {/if}
  {@render children()}
</button>
