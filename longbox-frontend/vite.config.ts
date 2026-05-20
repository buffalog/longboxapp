import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: false
      }
    }
  },
  // Under Vitest, resolve the browser build of Svelte so component
  // tests can `mount()`. Without this the SvelteKit plugin serves the
  // SSR build and @testing-library/svelte's render() throws
  // `lifecycle_function_unavailable` ("mount is not available on the
  // server").
  resolve: process.env.VITEST ? { conditions: ['browser'] } : {},
  test: {
    include: ['src/**/*.{test,spec}.{js,ts}'],
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./vitest.setup.ts']
  }
});
