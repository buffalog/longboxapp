<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import ScanStatusBadge from './ScanStatusBadge.svelte';

  type NavLink = { href: string; label: string };
  type NavItem =
    | { kind: 'link'; href: string; label: string }
    | { kind: 'menu'; label: string; children: NavLink[] };

  // Five top-level items — two grouping menus + three plain links.
  // `Library` clusters the catalog-health surfaces, `Releases` the
  // release / pull surfaces.
  const nav: NavItem[] = [
    { kind: 'link', href: '/', label: 'Dashboard' },
    {
      kind: 'menu',
      label: 'Library',
      children: [
        { href: '/series', label: 'Series' },
        { href: '/files', label: 'Files' },
        { href: '/missing', label: 'Missing' },
        { href: '/scans', label: 'Scans' },
        { href: '/library/tidy', label: 'Library tidy' },
        { href: '/needs-attention', label: 'Needs attention' }
      ]
    },
    {
      kind: 'menu',
      label: 'Releases',
      children: [
        { href: '/releases/calendar', label: 'Calendar' },
        { href: '/releases/pull-list', label: 'Pull list' }
      ]
    },
    { kind: 'link', href: '/add', label: 'Add' },
    { kind: 'link', href: '/settings', label: 'Settings' }
  ];

  // Label of the open dropdown, or null. One menu open at a time.
  let openMenu = $state<string | null>(null);
  let scrolled = $state(false);

  function isActive(href: string, pathname: string): boolean {
    if (href === '/') return pathname === '/';
    return pathname === href || pathname.startsWith(href + '/');
  }

  function menuActive(children: NavLink[], pathname: string): boolean {
    return children.some((c) => isActive(c.href, pathname));
  }

  function toggleMenu(label: string): void {
    openMenu = openMenu === label ? null : label;
  }

  function onWindowKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') openMenu = null;
  }

  // Close the open dropdown on any click outside a dropdown. A click on
  // a trigger or a menu item is inside `[data-nav-dropdown]`, so the
  // trigger's own toggle and an item's navigation are unaffected.
  function onWindowClick(e: MouseEvent): void {
    if (openMenu === null) return;
    const target = e.target as HTMLElement | null;
    if (!target?.closest('[data-nav-dropdown]')) openMenu = null;
  }

  // Shadow appears once the page has scrolled at all. `position: sticky`
  // has no native "has-scrolled" pseudo-class; a tiny JS listener does
  // it. Threshold 0 is fine for v1 — iOS overscroll can briefly dip
  // y<0, but the shadow vanishing for a frame is visually correct.
  onMount(() => {
    const onScroll = () => {
      scrolled = window.scrollY > 0;
    };
    onScroll();
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => window.removeEventListener('scroll', onScroll);
  });
</script>

<svelte:window onkeydown={onWindowKeydown} onclick={onWindowClick} />

<!-- `position: sticky; top: 0` keeps the nav in-flow but pinned during
     scroll. z-40 sits below Modal's z-50 and above the alpha scrubber's
     z-30; the dropdown menus, nested inside, layer above page content. -->
<nav
  class="sticky top-0 z-40 border-b border-slate-200 bg-white/85 backdrop-blur transition-shadow"
  class:shadow-sm={scrolled}
>
  <div class="mx-auto flex max-w-6xl items-center gap-6 px-4 py-3">
    <a href="/" class="text-base font-bold tracking-tight">LongBox</a>
    <ul class="flex items-center gap-1 text-sm">
      {#each nav as item (item.label)}
        <li class="relative" data-nav-dropdown={item.kind === 'menu' ? true : undefined}>
          {#if item.kind === 'link'}
            <a
              href={item.href}
              class="rounded-md px-2 py-1 hover:bg-slate-100"
              class:bg-slate-900={isActive(item.href, $page.url.pathname)}
              class:text-white={isActive(item.href, $page.url.pathname)}
              aria-current={isActive(item.href, $page.url.pathname) ? 'page' : undefined}
            >
              {item.label}
            </a>
          {:else}
            {@const active = menuActive(item.children, $page.url.pathname)}
            <button
              type="button"
              class="inline-flex items-center gap-1 rounded-md px-2 py-1 hover:bg-slate-100"
              class:bg-slate-900={active}
              class:text-white={active}
              aria-expanded={openMenu === item.label}
              aria-current={active ? 'true' : undefined}
              onclick={() => toggleMenu(item.label)}
            >
              {item.label}<span aria-hidden="true" class="text-[0.65em]">▾</span>
            </button>
            {#if openMenu === item.label}
              <!-- A disclosure, not an ARIA menu — the children are
                   ordinary navigation links, so they keep the `link`
                   role and the button uses `aria-expanded`. -->
              <ul
                class="absolute left-0 mt-1 min-w-44 rounded-md border border-slate-200 bg-white py-1 shadow-lg"
              >
                {#each item.children as child (child.href)}
                  <li>
                    <a
                      href={child.href}
                      class="block px-3 py-1.5 hover:bg-slate-100"
                      class:font-semibold={isActive(child.href, $page.url.pathname)}
                      class:text-slate-900={isActive(child.href, $page.url.pathname)}
                      aria-current={isActive(child.href, $page.url.pathname)
                        ? 'page'
                        : undefined}
                      onclick={() => (openMenu = null)}
                    >
                      {child.label}
                    </a>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
    <div class="ml-auto"><ScanStatusBadge /></div>
  </div>
</nav>
