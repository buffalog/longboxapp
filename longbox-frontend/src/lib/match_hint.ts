// Extracts a default search-query hint from a file's path. Picks the
// parent directory name (e.g. "Saga (2012)" → "Saga") and strips a
// trailing year-in-parens. Fallback chain ends with the basename.

export function searchHintFromPath(pathRelative: string): string {
  if (!pathRelative) return '';
  const parts = pathRelative.split('/').filter((s) => s.length > 0);
  // Walk parents from closest to root; first one that yields a usable
  // hint wins. The basename itself is skipped — too noisy.
  const parents = parts.slice(0, -1).reverse();
  for (const p of parents) {
    const cleaned = stripYearSuffix(p).trim();
    if (cleaned) return cleaned;
  }
  // No parents — fall back to the basename (stripped of extension).
  const base = parts.at(-1) ?? '';
  return stripYearSuffix(base.replace(/\.[^.]+$/, '')).trim();
}

function stripYearSuffix(name: string): string {
  // Strip a trailing " (YYYY)" or " (YYYY-YYYY)" or " (YYYY-)" group.
  return name.replace(/\s*\((?:19|20)\d{2}(?:[-\s]+(?:(?:19|20)\d{2})?)?\)\s*$/, '');
}
