import { getEnrichmentQueue, getEnrichmentSummary } from '$lib/api/enrichment';
import {
  listDuplicates,
  listKeptPhantoms,
  listPhantoms,
  listUntracked
} from '$lib/api/reconcile';

export const load = async () => {
  // All six reads in parallel — the page renders nothing useful
  // without all of them, and the slowest one (typically the
  // phantoms query, which joins files + issues) sets the lower
  // bound either way.
  const [phantoms, kept, untracked, enrichmentSummary, enrichmentQueue, duplicates] =
    await Promise.all([
      listPhantoms(),
      listKeptPhantoms(),
      listUntracked(),
      getEnrichmentSummary(),
      getEnrichmentQueue(),
      listDuplicates()
    ]);
  return {
    phantoms,
    kept,
    untracked,
    enrichmentSummary,
    enrichmentQueue,
    duplicates: duplicates.pairs
  };
};
