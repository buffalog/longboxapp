<script lang="ts">
  import { page } from '$app/stores';
  import ScanStatusBadge from './ScanStatusBadge.svelte';

  const links = [
    { href: '/', label: 'Dashboard' },
    { href: '/series', label: 'Series' },
    { href: '/add', label: 'Add' },
    { href: '/files', label: 'Files' },
    { href: '/scans', label: 'Scans' },
    { href: '/settings', label: 'Settings' }
  ];

  function isActive(href: string, pathname: string): boolean {
    if (href === '/') return pathname === '/';
    return pathname === href || pathname.startsWith(href + '/');
  }
</script>

<nav class="border-b border-slate-200 bg-white">
  <div class="mx-auto flex max-w-6xl items-center gap-6 px-4 py-3">
    <a href="/" class="text-base font-bold tracking-tight">LongBox</a>
    <ul class="flex items-center gap-1 text-sm">
      {#each links as link (link.href)}
        <li>
          <a
            href={link.href}
            class="rounded-md px-2 py-1 hover:bg-slate-100"
            class:bg-slate-900={isActive(link.href, $page.url.pathname)}
            class:text-white={isActive(link.href, $page.url.pathname)}
            aria-current={isActive(link.href, $page.url.pathname) ? 'page' : undefined}
          >
            {link.label}
          </a>
        </li>
      {/each}
    </ul>
    <div class="ml-auto"><ScanStatusBadge /></div>
  </div>
</nav>
