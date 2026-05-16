<script lang="ts">
  interface Props {
    label?: string;
    size?: 'sm' | 'md' | 'lg';
  }

  let { label = 'Loading…', size = 'md' }: Props = $props();

  const sizeClass = {
    sm: 'size-4 border-2',
    md: 'size-6 border-2',
    lg: 'size-8 border-[3px]'
  }[size];
</script>

<div class="inline-flex items-center gap-2 text-slate-500" role="status" aria-label={label}>
  <span
    class="inline-block animate-spin rounded-full border-current border-r-transparent {sizeClass}"
    aria-hidden="true"
  ></span>
  <span class="text-sm">{label}</span>
</div>
