<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import ScanStatusBadge from './ScanStatusBadge.svelte';

  const links = [
    { href: '/', label: 'Dashboard' },
    { href: '/series', label: 'Series' },
    { href: '/add', label: 'Add' },
    { href: '/files', label: 'Files' },
    { href: '/missing', label: 'Missing' },
    { href: '/scans', label: 'Scans' },
    // TODO Step 12 (A.8 nav restructure): fold into the nav restructure.
    // Flat link until then.
    { href: '/library/tidy', label: 'Library tidy' },
    // TODO Step 12 (nav restructure): fold into the `Releases ▾` dropdown
    // alongside Calendar / Releases of note. Flat link until then.
    { href: '/releases/pull-list', label: 'Pull list' },
    { href: '/settings', label: 'Settings' }
  ];

  function isActive(href: string, pathname: string): boolean {
    if (href === '/') return pathname === '/';
    return pathname === href || pathname.startsWith(href + '/');
  }

  // Shadow appears once the page has scrolled at all. `position: sticky`
  // has no native "has-scrolled" pseudo-class; tiny JS listener does it.
  // Threshold of 0 is fine for v1 — iOS rubber-band overscroll can briefly
  // dip into y<0, but the shadow vanishing for a frame is acceptable and
  // visually correct (we're momentarily at the top edge). Bumping to >4
  // is a future polish option if it surfaces as user-visible flicker.
  let scrolled = $state(false);

  onMount(() => {
    const onScroll = () => {
      scrolled = window.scrollY > 0;
    };
    onScroll(); // capture initial state (e.g., navigation to mid-page anchor)
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => window.removeEventListener('scroll', onScroll);
  });
</script>

<!-- `position: sticky; top: 0` keeps the nav in-flow but pinned to the
     viewport top during scroll. z-40 sits below Modal's z-50 so modal
     backdrops correctly cover the nav when open, and above the alpha
     scrubber's z-30. `bg-white/85 backdrop-blur` matches the brief's
     translucent-with-8px-blur treatment; opacity stacks as the fallback
     when backdrop-filter is unsupported, so plain-white-with-fade still
     looks intentional. -->
<nav
  class="sticky top-0 z-40 border-b border-slate-200 bg-white/85 backdrop-blur transition-shadow"
  class:shadow-sm={scrolled}
>
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
