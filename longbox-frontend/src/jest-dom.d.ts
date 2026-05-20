// Brings @testing-library/jest-dom's matcher types (toBeInTheDocument,
// etc.) into the TypeScript program so `svelte-check` / `tsc` recognise
// them on `expect(...)`. The runtime registration happens in
// `vitest.setup.ts`, but that file lives outside `src/` and so isn't
// part of the type-check program — this `.d.ts` (under `src/`) is.
import '@testing-library/jest-dom/vitest';
