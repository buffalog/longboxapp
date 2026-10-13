// Solicitation predicate — shared by the /series/[id] issue table and
// the /missing page so the two surfaces can't drift.
//
// A "solicited" issue is one whose release date (store_date, else
// cover_date) is today or in the future
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

/// The dates an issue carries. `store_date` is CV's on-sale date and is
/// optional so payloads that predate it still type-check.
export interface Dated {
  store_date?: string | null;
  cover_date: string | null;
}

/// Effective release date: the on-sale `store_date`, else `cover_date`
/// (which CV runs 1-2 months later). Twin of the server's
/// `COALESCE(store_date, cover_date)`.
export function releaseDate(issue: Dated): string | null {
  return issue.store_date ?? issue.cover_date;
}

/// True when the issue's release date is a fully-specified date that is
/// today or later. Takes the issue, not a date string, so a caller can't
/// hand it a bare `cover_date` and skip `store_date`.
///
/// CV dates are loose. Anything that isn't a clean `YYYY-MM-DD` — null,
/// partial, garbage — is treated as NOT solicited (it falls back to
/// plain "missing").
export function isSolicited(issue: Dated): boolean {
  const date = releaseDate(issue);
  if (!date || !FULL_DATE_RE.test(date)) return false;
  const release = Date.parse(date + 'T00:00:00Z');
  if (Number.isNaN(release)) return false;
  // Compare against the start of today (UTC) so an issue releasing
  // exactly today classifies as solicited.
  const now = new Date();
  const startOfToday = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());
  return release >= startOfToday;
}
