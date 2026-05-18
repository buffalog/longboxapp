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

/** Format a byte count as KB/MB/GB. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const kb = n / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  return `${(mb / 1024).toFixed(2)} GB`;
}
