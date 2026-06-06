use std::str::FromStr;

use sqlx::SqliteExecutor;

use crate::error::Result;

/// `settings.key` value: user-tunable owned/needs_review boundary.
pub const KEY_MATCH_CONFIDENCE_THRESHOLD: &str = "match_confidence_threshold";

/// `settings.key` value: filesystem path of the library root.
pub const KEY_LIBRARY_ROOT_PATH: &str = "library_root_path";

/// `settings.key` value: WAL checkpoint interval in hours. A
/// background task in the web crate sleeps this long, then runs
/// `PRAGMA wal_checkpoint(TRUNCATE)` to keep the WAL file from
/// growing unbounded under heavy enrichment / scan loads. Default
/// 6 hours. Read once per loop iteration so a change tunes the next
/// sleep without a restart. Set to 0 to disable.
pub const KEY_WAL_CHECKPOINT_HOURS: &str = "wal_checkpoint_hours";

/// `settings.key` value: optional host-side library path used by the
/// "Show in Finder" affordance. The container only sees its
/// `library_root_path` (e.g. `/library`); to render a `file://` URL
/// that opens on the user's host OS, we substitute that prefix with
/// this value. Empty/unset → frontend renders the container path
/// (informational but not openable from a browser). Stored as-is;
/// the validator is a no-op because the container can't introspect
/// the host filesystem.
pub const KEY_HOST_LIBRARY_PATH: &str = "host_library_path";

/// `settings.key` value: auto-tidy master switch (`'true'` / `'false'`).
/// When off, the scanner still ticks `series.consecutive_empty_scans`
/// but never marks a series for automatic removal.
pub const KEY_AUTO_TIDY_ENABLED: &str = "auto_tidy_enabled";

/// `settings.key` value: directory all backups land in (pre-delete
/// snapshots today; future internalized periodic snapshots if/when
/// they ship). Empty/unset → same directory as the DB file
/// (historical behavior). Validated at write time as an existing
/// writable directory — the backup writer assumes it stays valid
/// until the next write.
pub const KEY_BACKUP_PATH: &str = "backup_path";

/// `settings.key` value: pull-engine pre-grab series-title similarity
/// threshold (Bug 3). Pull sweep reads this per-sweep, so tuning needs
/// no restart. Default `longbox_core::PULL_INDEXER_MATCH_THRESHOLD`
/// (0.75) — strictness vs the catalog matcher justified by asymmetric
/// recovery cost of a wrong NZB grab.
pub const KEY_PULL_INDEXER_MATCH_THRESHOLD: &str = "pull_indexer_match_threshold";

/// `settings.key` value: comma-separated list of release-title
/// substrings the pre-grab filter silently drops. Used to keep
/// digital-only formats (Marvel "Infinity Comic", DC "Infinite
/// Comic") out of the library. Pull sweep reads this per-sweep so
/// editing the row tunes the next sweep without a restart.
pub const KEY_PULL_EXCLUSION_KEYWORDS: &str = "pull_exclusion_keywords";

/// CV enrichment tunables (Step 6c.1). Worker reads each per-cycle so
/// flipping a value tunes the next attempt with no restart. Defaults
/// are the conservative-to-a-fault choices from the kickoff and
/// codified in the 20260529 migration.
pub const KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_KNOWN: &str =
    "cv_enrichment_title_threshold_year_known";
pub const KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_UNKNOWN: &str =
    "cv_enrichment_title_threshold_year_unknown";
pub const KEY_CV_ENRICHMENT_COUNT_WINDOW_YEAR_KNOWN: &str =
    "cv_enrichment_count_window_year_known";
pub const KEY_CV_ENRICHMENT_COUNT_WINDOW_YEAR_UNKNOWN: &str =
    "cv_enrichment_count_window_year_unknown";
pub const KEY_CV_ENRICHMENT_DOMINANT_GAP: &str = "cv_enrichment_dominant_gap";
pub const KEY_CV_ENRICHMENT_COOLDOWN_DAYS: &str = "cv_enrichment_cooldown_days";
pub const KEY_CV_ENRICHMENT_REQUEST_INTERVAL_SECONDS: &str =
    "cv_enrichment_request_interval_seconds";
/// Bounded-sample gate enforcing the 6c.3 observation step
/// structurally. Worker stops after this many attempts per cycle;
/// 0 disables the bound (full backlog).
pub const KEY_CV_ENRICHMENT_MAX_RUN: &str = "cv_enrichment_max_run";

/// Bug 4a kill switch. Worker reads per-cycle and idles when
/// false. Code default is true (forward-compatible); the Bug 4a
/// migration inserts the row with 'false' to hold the worker down
/// while the Bug 4 padding-mismatch repair lands. Flip with
/// `UPDATE settings SET value='true' WHERE key='cv_enrichment_enabled'`
/// to resume; takes effect within one config-read iteration, no
/// restart needed.
pub const KEY_CV_ENRICHMENT_ENABLED: &str = "cv_enrichment_enabled";

// -------- Item A v2: Metron forward-calendar integration --------

/// Kill switch for the Metron forward-calendar path. Default 'false'
/// (set by the 20260601 migration) so the feature is inert until the
/// deploy env carries valid credentials AND the row is flipped to
/// 'true'. Read once at boot — `bootstrap` skips MetronClient
/// construction entirely when this is false, so a disabled Metron
/// incurs no env-var reads, no settings reads, no warn logs.
pub const KEY_METRON_ENABLED: &str = "metron_enabled";

/// How many forward weeks the calendar handler asks Metron about on
/// "Next week" navigation. Default 4 (set by migration). Read lazily
/// per forward-window calendar request — pattern mirrors
/// `EnrichmentConfig::load`, allows live tuning via sidecar without a
/// container restart.
pub const KEY_METRON_CALENDAR_FORWARD_WEEKS: &str = "metron_calendar_forward_weeks";

/// TTL on the `metron_calendar_cache` rows. Default 24 hours (set by
/// migration). Forward solicitation data shifts (FOC cancellations,
/// date moves) but not minute-by-minute — 24h balances freshness
/// against API calls. Read lazily per-request.
pub const KEY_METRON_CALENDAR_CACHE_TTL_HOURS: &str = "metron_calendar_cache_ttl_hours";

// -------- Phase B virtiofs polling fallback --------

/// Poll interval for the Phase B watcher (whole seconds). Default 30
/// (set by the 20260601020000 migration). Inside Docker Desktop's
/// virtiofs mount inotify is blind to host-written files, so the
/// watcher uses `notify::PollWatcher` instead of RecommendedWatcher;
/// this row controls how often the poll runs. Read once at boot —
/// changing it requires a container restart.
pub const KEY_PHASE_B_POLL_INTERVAL_SECONDS: &str = "phase_b_poll_interval_seconds";

/// Minimum file size in MEGABYTES the Phase B processor accepts.
/// Anything smaller is presumed a partial/corrupt SAB delivery
/// (e.g. an `Absolute Batman` issue that should be 50+ MB arriving
/// at 16 MB with five rendered pages) and gets rejected: WARN log,
/// no DB row, file stays in /watch/ so the operator can investigate
/// or replace the broken download. Default 35 — catches the most
/// common pad-and-pray failure modes while still allowing legitimate
/// ashcans / single-issue digital comics that come in around 30-50
/// MB. Phase B reads per-file so tuning needs no restart.
pub const KEY_MIN_FILE_SIZE_MB: &str = "min_file_size_mb";

pub async fn get<'e, E>(executor: E, key: &str) -> Result<Option<String>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(r#"SELECT value FROM settings WHERE key = ?"#, key)
        .fetch_optional(executor)
        .await?;
    Ok(row.map(|r| r.value))
}

/// Upsert: set the value for `key`, replacing any prior value. `updated_at`
/// is bumped to the current timestamp.
pub async fn set<'e, E>(executor: E, key: &str, value: &str) -> Result<()>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO settings (key, value) VALUES (?, ?)
           ON CONFLICT(key) DO UPDATE
           SET value = excluded.value, updated_at = CURRENT_TIMESTAMP"#,
        key,
        value
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Read `key` and parse via `FromStr`. Returns `default` when the row is
/// missing or its value fails to parse — the matcher's threshold setting
/// should never panic the scanner because someone wrote `'banana'` into the
/// table.
pub async fn get_or_default<'e, E, T>(executor: E, key: &str, default: T) -> Result<T>
where
    E: SqliteExecutor<'e>,
    T: FromStr,
{
    match get(executor, key).await? {
        Some(raw) => Ok(raw.parse::<T>().unwrap_or(default)),
        None => Ok(default),
    }
}
