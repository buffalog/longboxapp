<script lang="ts">
  import { Folder, Search } from 'lucide-svelte';
  import Button from './Button.svelte';

  interface Props {
    folder: string;
    count: number;
    busy?: boolean;
    onOpen: (folder: string) => void;
  }

  let { folder, count, busy = false, onOpen }: Props = $props();
</script>

<article class="flex items-center gap-3 rounded-lg border border-slate-200 bg-white p-3">
  <Folder class="size-5 flex-shrink-0 text-slate-400" aria-hidden="true" />
  <div class="min-w-0 flex-1">
    <div class="truncate font-medium" title={folder}>{folder}</div>
    <div class="text-xs text-slate-500">
      {count} actionable file{count === 1 ? '' : 's'}
    </div>
  </div>
  <Button onclick={() => onOpen(folder)} loading={busy} size="sm">
    <Search class="size-3.5" aria-hidden="true" />
    Search ComicVine
  </Button>
</article>
