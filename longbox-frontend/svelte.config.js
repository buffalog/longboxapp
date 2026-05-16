import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      // Build straight into longbox-web/frontend-dist/ so rust-embed picks
      // it up at compile time. Sibling-directory coupling is intentional.
      pages: '../longbox-web/frontend-dist',
      assets: '../longbox-web/frontend-dist',
      fallback: 'index.html',
      strict: true
    }),
    prerender: { entries: [] }
  }
};

export default config;
