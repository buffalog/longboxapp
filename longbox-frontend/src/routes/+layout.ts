// Pure SPA. No SSR (no Node runtime in the rust-embed bundle) and no
// prerender (data comes from the live backend at runtime). adapter-static's
// `fallback: 'index.html'` makes every path resolve to the SPA shell, which
// hydrates and runs +page.ts load functions in the browser.
export const ssr = false;
export const prerender = false;
export const trailingSlash = 'never';
