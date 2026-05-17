// Pure SPA. No SSR (no Node runtime in the rust-embed bundle) and no
// prerender (data comes from the live backend at runtime). adapter-static's
// `fallback: 'index.html'` makes every path resolve to the SPA shell, which
// hydrates and runs +page.ts load functions in the browser.
export const ssr = false;
export const prerender = false;
export const trailingSlash = 'never';

import { listLibraryRoots } from '$lib/api/library_roots';

/** Fetch the configured library roots once at app boot. Children access via
 *  `data.libraryRoot` (SvelteKit merges layout data into page props).
 *  Phase A always returns exactly one element from /api/library-roots;
 *  multi-root is Phase B work that would change `libraryRoot` to a list. */
export const load = async () => {
  try {
    const roots = await listLibraryRoots();
    return { libraryRoot: roots[0] ?? null };
  } catch {
    return { libraryRoot: null };
  }
};
