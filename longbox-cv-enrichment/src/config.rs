//! Per-cycle configuration read from the settings table. Every
//! enrichment cycle starts by calling [`EnrichmentConfig::load`]
//! so a setting change tunes the next cycle without a restart —
//! same hygiene as the pull engine's `pull_indexer_match_threshold`.

use longbox_comicvine::EnrichmentThresholds;
use longbox_db::{settings_repo, Pool};

#[derive(Debug, Clone)]
pub struct EnrichmentConfig {
    /// Bug 4a kill switch. `false` makes the worker idle on every
    /// cycle without attempting any series. Code default is `true`
    /// (forward-compatible for future fresh deploys); the Bug 4a
    /// migration row holds it down at `false` for the Bug 4 deploy
    /// specifically.
    pub enabled: bool,
    pub thresholds: EnrichmentThresholds,
    pub cooldown_days: i64,
    pub request_interval_seconds: u64,
    pub max_run: i64,
    pub first_run_sample_mode: bool,
    pub first_run_easy_quota: i64,
    pub first_run_discipline_quota: i64,
    pub first_run_collision_quota: i64,
}

impl EnrichmentConfig {
    pub async fn load(db: &Pool) -> Result<Self, longbox_db::DbError> {
        // Bug 4a: kill-switch read FIRST. Code default `true` —
        // a fresh deploy with no settings row enables the worker.
        // The Bug 4a migration inserts an explicit 'false' row
        // for this deploy.
        let enabled: bool =
            settings_repo::get_or_default(db, settings_repo::KEY_CV_ENRICHMENT_ENABLED, true)
                .await?;

        let title_known: f64 = settings_repo::get_or_default(
            db,
            settings_repo::KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_KNOWN,
            0.85,
        )
        .await?;
        let title_unknown: f64 = settings_repo::get_or_default(
            db,
            settings_repo::KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_UNKNOWN,
            0.95,
        )
        .await?;
        let count_known: f64 = settings_repo::get_or_default(
            db,
            settings_repo::KEY_CV_ENRICHMENT_COUNT_WINDOW_YEAR_KNOWN,
            0.20,
        )
        .await?;
        let count_unknown: f64 = settings_repo::get_or_default(
            db,
            settings_repo::KEY_CV_ENRICHMENT_COUNT_WINDOW_YEAR_UNKNOWN,
            0.10,
        )
        .await?;
        let dominant_gap: f64 =
            settings_repo::get_or_default(db, settings_repo::KEY_CV_ENRICHMENT_DOMINANT_GAP, 0.20)
                .await?;
        let cooldown_days: i64 = settings_repo::get_or_default(
            db,
            settings_repo::KEY_CV_ENRICHMENT_COOLDOWN_DAYS,
            7,
        )
        .await?;
        let request_interval_seconds: u64 = settings_repo::get_or_default(
            db,
            settings_repo::KEY_CV_ENRICHMENT_REQUEST_INTERVAL_SECONDS,
            30,
        )
        .await?;
        let max_run: i64 =
            settings_repo::get_or_default(db, settings_repo::KEY_CV_ENRICHMENT_MAX_RUN, 20).await?;

        // The first-run sample defaults to ON the first time the
        // worker runs against a fresh deploy. The toggle is
        // separate from the keys above so 6c.3 can flip it
        // independently of tuning thresholds.
        let first_run_sample_mode: bool = settings_repo::get_or_default(
            db,
            "cv_enrichment_first_run_sample_mode",
            true,
        )
        .await?;
        let first_run_easy_quota: i64 =
            settings_repo::get_or_default(db, "cv_enrichment_first_run_easy_quota", 10).await?;
        let first_run_discipline_quota: i64 =
            settings_repo::get_or_default(db, "cv_enrichment_first_run_discipline_quota", 10)
                .await?;
        let first_run_collision_quota: i64 =
            settings_repo::get_or_default(db, "cv_enrichment_first_run_collision_quota", 5).await?;

        Ok(Self {
            enabled,
            thresholds: EnrichmentThresholds {
                title_threshold_year_known: title_known,
                title_threshold_year_unknown: title_unknown,
                count_window_year_known: count_known,
                count_window_year_unknown: count_unknown,
                dominant_gap,
            },
            cooldown_days,
            request_interval_seconds,
            max_run,
            first_run_sample_mode,
            first_run_easy_quota,
            first_run_discipline_quota,
            first_run_collision_quota,
        })
    }
}
