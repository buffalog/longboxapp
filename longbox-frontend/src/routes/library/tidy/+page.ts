import { getEnrichmentQueue, getEnrichmentSummary } from '$lib/api/enrichment';
import { listPhantoms, listUntracked } from '$lib/api/reconcile';

export const load = async () => {
  // All four reads in parallel — the page renders nothing useful
  // without all of them, and the slowest one (typically the
  // phantoms query, which joins files + issues) sets the lower
  // bound either way.
  const [phantoms, untracked, enrichmentSummary, enrichmentQueue] = await Promise.all([
    listPhantoms(),
    listUntracked(),
    getEnrichmentSummary(),
    getEnrichmentQueue()
  ]);
  return { phantoms, untracked, enrichmentSummary, enrichmentQueue };
};
