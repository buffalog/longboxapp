// Display formatters. All pure functions; no DOM or fetch dependencies.

// Backend serializes time::PrimitiveDateTime via the Display impl, which
// produces "YYYY-MM-DD HH:MM:SS[.fff]" (space separator, no timezone).
// SQLite stores values in UTC, so we normalize: replace the space with "T"
// and append "Z" if there's no offset, then parse. Without this, Safari
// rejects the string outright and Chrome interprets it as local time.
function parseUtc(s: string): Date {
  const tIso = s.replace(' ', 'T');
  const hasTz = /Z$|[+\-]\d{2}:?\d{2}$/.test(tIso);
  return new Date(hasTz ? tIso : `${tIso}Z`);
}

/** Format an ISO 8601 timestamp as a short, human-friendly "YYYY-MM-DD HH:MM" UTC. */
export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = parseUtc(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => n.toString().padStart(2, '0');
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
}

/** Format a date-only string ("YYYY-MM-DD"). */
export function formatDate(s: string | null | undefined): string {
  return s ?? '—';
}

/** Relative time: "5m ago", "3h ago", "2d ago". Falls back to ISO. */
export function formatRelative(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = parseUtc(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const diff = Date.now() - d.getTime();
  if (diff < 0) return formatDateTime(iso);
  const s = Math.floor(diff / 1000);
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const dy = Math.floor(h / 24);
  return `${dy}d ago`;
}

/** Format a confidence score as a percentage. */
export function formatConfidence(c: number): string {
  return `${Math.round(c * 100)}%`;
}

/** Stable label for a MatchMethod enum value. */
export function formatMatchMethod(m: string): string {
  switch (m) {
    case 'web_url_cv':
      return 'CV URL';
    case 'web_url_metron':
      return 'Metron URL';
    case 'comicinfo_xml':
      return 'ComicInfo';
    case 'filename_regex':
      return 'Filename';
    case 'manual':
      return 'Manual';
    case 'phase_b':
      return 'Imported';
    case 'unmatched':
      return 'Unmatched';
    case 'ignored':
      return 'Ignored';
    default:
      return m;
  }
}

/** Format a duration in ms as "<1s", "Xs", "Xm Ys", or "Xh Ym". Sub-second
 *  durations clamp to "<1s" — a no-op rescan rendering as "0ms" reads as
 *  a bug, and exact-millisecond resolution doesn't carry useful signal on
 *  a scan card. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return '<1s';
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rs = s % 60;
  if (m < 60) return rs ? `${m}m ${rs}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return rm ? `${h}h ${rm}m` : `${h}h`;
}

/**
 * Format a byte count with BINARY units — KiB/MiB/GiB, powers of 1024.
 *
 * The labels used to read KB/MB/GB while the arithmetic divided by 1024, so
 * every size in the app was binary maths wearing a decimal label: 3,872,304,022
 * bytes rendered as "3.61 GB" when 3.61 is the GiB figure and the GB figure is
 * 3.87. Relabelling rather than switching to powers of 1000 keeps every
 * displayed number exactly as it was — this is a correctness fix to the label,
 * not a change to what is shown.
 *
 * Worth knowing when comparing against the OS: macOS Finder reports decimal,
 * so deleting 3.61 GiB of duplicates shows as roughly 3.87 GB freed. Moving to
 * decimal would align with that, but it is a product choice affecting every
 * size in five views, not a bug fix.
 */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const kib = n / 1024;
  if (kib < 1024) return `${kib.toFixed(1)} KiB`;
  const mib = kib / 1024;
  if (mib < 1024) return `${mib.toFixed(1)} MiB`;
  return `${(mib / 1024).toFixed(2)} GiB`;
}

// ComicVine canonical URL construction. CV's permalinks use numeric
// prefixes per resource type — 4000 for issues, 4050 for volumes
// (their term for what we call series). Matches the same pattern
// Phase B's processor writes into ComicInfo `<Web>` (see
// longbox-postprocess/src/processor.rs::compose_metadata).

/** Canonical ComicVine URL for an issue. */
export function cvIssueUrl(cv_issue_id: number): string {
  return `https://comicvine.gamespot.com/issue/4000-${cv_issue_id}/`;
}

/** Canonical ComicVine URL for a series (CV's "volume" resource). */
export function cvSeriesUrl(cv_id: number): string {
  return `https://comicvine.gamespot.com/volume/4050-${cv_id}/`;
}
