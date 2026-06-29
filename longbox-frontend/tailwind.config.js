import typography from '@tailwindcss/typography';

/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{html,svelte,ts,js}'],
  theme: {
    extend: {
      colors: {
        // Status palette — paired with icons / text in components so colour
        // is never the sole indicator.
        status: {
          owned: '#16a34a', // green-600
          needs_review: '#d97706', // amber-600
          unmatched: '#dc2626', // red-600
          ignored: '#6b7280', // gray-500
          missing: '#9333ea', // purple-600 — an issue that should exist but isn't owned
          solicited: '#0284c7', // sky-600 — informational, not a problem state
          complete: '#9333ea' // purple-600 — a finished run, fully owned (series-card celebration)
        }
      }
    }
  },
  plugins: [typography]
};
