use std::str::FromStr;

use sqlx::SqliteExecutor;

use crate::error::Result;

/// `settings.key` value: user-tunable owned/needs_review boundary.
pub const KEY_MATCH_CONFIDENCE_THRESHOLD: &str = "match_confidence_threshold";

/// `settings.key` value: filesystem path of the library root.
pub const KEY_LIBRARY_ROOT_PATH: &str = "library_root_path";

/// `settings.key` value: auto-tidy master switch (`'true'` / `'false'`).
/// When off, the scanner still ticks `series.consecutive_empty_scans`
/// but never marks a series for automatic removal.
pub const KEY_AUTO_TIDY_ENABLED: &str = "auto_tidy_enabled";

/// `settings.key` value: pull-engine pre-grab series-title similarity
/// threshold (Bug 3). Pull sweep reads this per-sweep, so tuning needs
/// no restart. Default `longbox_core::PULL_INDEXER_MATCH_THRESHOLD`
/// (0.75) — strictness vs the catalog matcher justified by asymmetric
/// recovery cost of a wrong NZB grab.
pub const KEY_PULL_INDEXER_MATCH_THRESHOLD: &str = "pull_indexer_match_threshold";

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
