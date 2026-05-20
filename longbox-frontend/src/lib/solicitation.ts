// Solicitation predicate — shared by the /series/[id] issue table and
// the /missing page so the two surfaces can't drift.
//
// A "solicited" issue is one whose cover_date is today or in the future
// (it hasn't shipped / can't be in hand yet). Phase A.8 renders these
// distinctly from plain "missing" (past-dated, never acquired).
//
// today-inclusive per the Phase A.8 brief's `cover_date >= CURRENT_DATE`
// predicate — an issue cover-dated today still counts as solicited.

/// Strict `YYYY-MM-DD` shape. CV cover dates are loose — null, full
/// dates, or partial (`YYYY`, `YYYY-MM`). We require a fully-specified
/// day: a partial date isn't a precise enough predicate, and
/// `Date.parse` is lenient enough to (mis)parse bare years.
const FULL_DATE_RE = /^\d{4}-\d{2}-\d{2}$/;

/// True when `cover_date` is a fully-specified date that is today or
/// later.
///
/// `cover_date` is CV-supplied and loose. Anything that isn't a clean
/// `YYYY-MM-DD` — null, partial, garbage — is treated as NOT solicited
/// (it falls back to plain "missing").
export function isSolicited(coverDate: string | null | undefined): boolean {
  if (!coverDate || !FULL_DATE_RE.test(coverDate)) return false;
  const cover = Date.parse(coverDate + 'T00:00:00Z');
  if (Number.isNaN(cover)) return false;
  // Compare against the start of today (UTC) so an issue cover-dated
  // exactly today classifies as solicited.
  const now = new Date();
  const startOfToday = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());
  return cover >= startOfToday;
}
