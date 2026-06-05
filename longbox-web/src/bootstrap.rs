//! Startup orchestration. Builds [`AppState`] from [`AppConfig`].

use std::sync::Arc;
use std::time::Duration;

use longbox_comicvine::{ComicVineClient, ComicVineClientConfig};
use longbox_db::{library_root_repo, settings_repo, NewLibraryRoot};
use longbox_metron::{MetronClient, MetronClientConfig};
use longbox_scanner::{Scanner, ScannerConfig};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::{info, warn};
use ulid::Ulid;

use crate::config::{normalize_path, AppConfig};
use crate::state::{AppState, CurrentScan, ScanKind, ScanStatus};

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error(transparent)]
    Db(#[from] longbox_db::DbError),

    #[error("CV client init failed: {0}")]
    Cv(longbox_comicvine::CvError),

    #[error(
        "library_roots row exists with path {existing:?} but configured path is {configured:?}. \
         LongBox refuses to silently mutate the library root because doing so would orphan the file catalog. \
         Either fix LIBRARY_ROOT_PATH to match the existing row, or manually update the DB."
    )]
    LibraryRootMismatch {
        existing: String,
        configured: String,
    },
}

pub async fn run(config: AppConfig) -> Result<AppState, BootstrapError> {
    // 1 + 2. Pool open via longbox_db so production pragmas (WAL, FK=ON,
    //        synchronous=NORMAL, busy_timeout=5s) and migrations both apply.
    let db = longbox_db::open(&config.database_url).await?;
    info!(target: "longbox_web", db = %config.database_url, "database pool opened");

    // 3. Upsert library_roots row from config.library_root_path.
    let configured = normalize_path(&config.library_root_path);
    let existing_rows = library_root_repo::list_all(&db).await?;
    let library_root_id = match existing_rows.as_slice() {
        [] => {
            let row = library_root_repo::insert(
                &db,
                NewLibraryRoot {
                    path: configured.clone(),
                },
            )
            .await?;
            info!(
                target: "longbox_web",
                id = row.id,
                path = %configured,
                "library_roots row created"
            );
            row.id
        }
        [existing] => {
            let existing_norm = normalize_path(&existing.path);
            if existing_norm != configured {
                return Err(BootstrapError::LibraryRootMismatch {
                    existing: existing.path.clone(),
                    configured,
                });
            }
            existing.id
        }
        many => {
            // Phase A models a single library root. More than one is a
            // hand-edited DB; treat the first one as authoritative for the
            // mismatch check and surface as a normal mismatch error.
            let first = &many[0];
            if normalize_path(&first.path) != configured {
                return Err(BootstrapError::LibraryRootMismatch {
                    existing: first.path.clone(),
                    configured,
                });
            }
            first.id
        }
    };

    // 4. ComicVine client.
    let cv = ComicVineClient::new(ComicVineClientConfig {
        api_key: config.comicvine_api_key.clone(),
        timeout: Duration::from_secs(30),
        rate_limit_per_hour: 180,
        max_wait_for_slot: Duration::from_secs(60),
        ..Default::default()
    })
    .map_err(BootstrapError::Cv)?;

    // 5. Scanner.
    let scanner = Arc::new(Scanner::new(
        db.clone(),
        ScannerConfig {
            match_threshold: config.match_threshold,
        },
    ));

    // 6. Phase B pending-intervention cache. Created here so it exists
    //    even when the watcher is disabled — the dashboard handler
    //    reads from it unconditionally and returns 0 when empty.
    let pending_cache = Arc::new(longbox_postprocess::PendingInterventionsCache::new());

    // 7. Phase B post-process watcher (enabled implicitly via env).
    //    `DOWNLOAD_WATCH_PATH` set + dir is readable  -> start watcher.
    //    Unset                                        -> single info log, skip.
    //    Set but unreadable / start() failure         -> warn, skip.
    //    Web boot is unaffected either way — Phase B is best-effort.
    start_phase_b(&config, &db, Arc::clone(&pending_cache)).await;

    // 8. Pull engine — the Phase A.8 scheduled auto-download workflow.
    //    Always started: the scheduler is a cheap sleeping task and
    //    each sweep no-ops gracefully when no downloader or indexers
    //    are configured.
    let pull = longbox_pull::start(
        longbox_pull::PullConfig {
            daily_time: config.pull_schedule_time,
        },
        db.clone(),
    );
    // On-demand single-series search tracker. Spawns its own short-
    // lived tokio task per accepted search; the pool clone is what
    // they run against.
    let pull_search = longbox_pull::PullSearchHandle::new(db.clone());

    // 9. Library Tidy scheduled scan. The scan-with-status logic is a
    //    closure built here — it needs `scan_status`, a web-layer type
    //    the scheduler crate can't depend on without a cycle; the
    //    generic scheduler just fires the closure daily.
    let scan_status = Arc::new(RwLock::new(ScanStatus::default()));
    let scan_scheduler = longbox_scan_scheduler::start(
        longbox_scan_scheduler::ScanSchedulerConfig {
            daily_time: config.scan_schedule_time,
        },
        {
            let scanner = Arc::clone(&scanner);
            let scan_status = Arc::clone(&scan_status);
            move || {
                run_scheduled_scan(
                    Arc::clone(&scanner),
                    Arc::clone(&scan_status),
                    library_root_id,
                )
            }
        },
    );

    // 10. CV enrichment worker (Step 6c.2). Performs its own
    //     startup migration assertion before doing any work. If the
    //     6c.1 columns are missing on the live DB, the worker
    //     refuses to start, logs at ERROR, and the returned handle's
    //     request_run() is a no-op — the rest of the web layer
    //     continues running.
    let cv_arc = Arc::new(cv);

    // Direct/interactive CV client — higher rate limit, short wait-for-slot.
    // Used by user-initiated routes (Library Tidy cv-id picks) so they never
    // queue behind the enrichment worker's 30-60s token wait.
    let cv_direct = ComicVineClient::new(ComicVineClientConfig {
        api_key: config.comicvine_api_key.clone(),
        timeout: Duration::from_secs(30),
        rate_limit_per_hour: 3_600,
        max_wait_for_slot: Duration::from_secs(3),
        ..Default::default()
    })
    .map_err(BootstrapError::Cv)?;
    let cv_direct_arc = Arc::new(cv_direct);
    let enrichment = longbox_cv_enrichment::spawn(
        db.clone(),
        Arc::clone(&cv_arc),
        std::path::PathBuf::from(&configured),
    );

    // 11. Metron client (Item A v2). Kill switch is the construction
    //     gate: `metron_enabled = false` (the migration default) skips
    //     construction entirely so a disabled feature incurs no env-var
    //     reads and no warn logs. `enabled = true` + bad credentials
    //     degrades to `None` with a warn — the route handler returns
    //     empty for forward weeks in that state, just like disabled.
    let metron_enabled =
        settings_repo::get_or_default(&db, settings_repo::KEY_METRON_ENABLED, false).await?;
    let metron = build_metron_client(
        metron_enabled,
        config.metron_api_user.as_deref(),
        config.metron_api_password.as_deref(),
    );

    // 12. WAL checkpoint background task. SQLite's WAL grows
    //     unbounded across heavy enrichment / scan sweeps unless
    //     periodically truncated. The default `PRAGMA
    //     wal_checkpoint(PASSIVE)` SQLite runs implicitly only
    //     consolidates pages, doesn't shrink the file. TRUNCATE
    //     shrinks it back to zero (when no readers are active).
    //     Reads the interval from settings every loop so a tune
    //     takes effect on the next sleep without a restart; 0 hours
    //     disables the loop entirely (returns early on the next
    //     iteration). Default 6h.
    spawn_wal_checkpoint(db.clone());

    // 13. Compose state.
    Ok(AppState {
        db,
        cv: cv_arc,
        cv_direct: cv_direct_arc,
        metron,
        scanner,
        config: Arc::new(config),
        scan_status,
        library_root_id,
        pending_cache,
        pull,
        pull_search,
        scan_scheduler,
        enrichment,
        start_time: time::OffsetDateTime::now_utc(),
    })
}

/// Fire-and-forget periodic `PRAGMA wal_checkpoint(TRUNCATE)`. Each
/// iteration: read the configured interval (`wal_checkpoint_hours`,
/// default 6) from settings, sleep that long, then checkpoint and
/// log the outcome at INFO. Setting the interval to 0 turns the loop
/// into a long-sleep no-op (10 minutes) so the loop is still picking
/// up settings changes; the next non-zero setting resumes the normal
/// rhythm.
///
/// The checkpoint result tuple `(busy, log_pages, checkpointed_pages)`
/// is what SQLite's `wal_checkpoint(TRUNCATE)` returns: `busy=1` when
/// the checkpoint couldn't complete because of an active reader (we
/// log and retry next iteration). `log_pages` is the WAL frame count
/// pre-checkpoint; `checkpointed_pages` is how many were written back
/// to the main DB.
fn spawn_wal_checkpoint(db: longbox_db::Pool) {
    use longbox_db::settings_repo;
    tokio::spawn(async move {
        loop {
            let hours: i64 = settings_repo::get_or_default(
                &db,
                settings_repo::KEY_WAL_CHECKPOINT_HOURS,
                6_i64,
            )
            .await
            .unwrap_or(6);
            if hours <= 0 {
                // Disabled — long-sleep so a tune (settings flip back
                // to a positive number) is picked up within minutes.
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                continue;
            }
            let sleep_secs = u64::try_from(hours)
                .unwrap_or(6)
                .saturating_mul(3600);
            tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;

            // PRAGMA wal_checkpoint returns one row: (busy, log,
            // checkpointed). All three columns are integers. Use
            // query_as with a tuple type.
            match sqlx::query_as::<_, (i64, i64, i64)>("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(&db)
                .await
            {
                Ok((busy, log_pages, checkpointed_pages)) => {
                    tracing::info!(
                        target: "longbox_web",
                        busy,
                        log_pages,
                        checkpointed_pages,
                        interval_hours = hours,
                        "WAL checkpoint complete"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "longbox_web",
                        error = %e,
                        "WAL checkpoint failed"
                    );
                }
            }
        }
    });
}

/// Construct the Metron client for [`AppState::metron`], collapsing all
/// disabled / degraded states to `None`. Extracted from the bootstrap
/// run() for unit-testability — the three pathways (disabled, bad
/// credentials, ok) are exercised in tests below.
fn build_metron_client(
    enabled: bool,
    user: Option<&str>,
    password: Option<&str>,
) -> Option<Arc<MetronClient>> {
    if !enabled {
        return None;
    }
    let config = MetronClientConfig {
        username: user.unwrap_or_default().to_owned(),
        password: password.unwrap_or_default().to_owned(),
        ..Default::default()
    };
    match MetronClient::new(config) {
        Ok(c) => Some(Arc::new(c)),
        Err(e) => {
            warn!(
                target: "longbox_web",
                error = %e,
                "metron credentials missing or invalid — forward calendar disabled"
            );
            None
        }
    }
}

/// One scheduled-scan attempt. Routes through the same `scan_status`
/// guard the manual scan route uses, so a scheduled scan and a manual
/// scan are mutually exclusive and a scheduled scan shows in the live
/// "scan in progress" indicator. A fire while a scan is already running
/// is skipped (logged) — the next day's slot retries.
async fn run_scheduled_scan(
    scanner: Arc<Scanner>,
    scan_status: Arc<RwLock<ScanStatus>>,
    library_root_id: i64,
) {
    {
        let mut status = scan_status.write().await;
        if status.current.is_some() {
            info!(
                target: "longbox_web",
                "scheduled scan skipped — a scan is already in progress"
            );
            return;
        }
        status.current = Some(CurrentScan {
            scan_id: Ulid::new().to_string(),
            library_root_id,
            kind: ScanKind::Full,
            started_at: OffsetDateTime::now_utc(),
        });
    }
    let result = scanner.scan_full(library_root_id).await;
    scan_status.write().await.current = None;
    match result {
        Ok(report) => info!(
            target: "longbox_web",
            files_seen = report.files_seen,
            "scheduled scan complete"
        ),
        Err(e) => warn!(target: "longbox_web", err = %e, "scheduled scan failed"),
    }
}

/// Phase B startup. Best-effort: a watch-folder misconfiguration must
/// not prevent the web layer from booting.
async fn start_phase_b(
    config: &AppConfig,
    db: &longbox_db::Pool,
    cache: Arc<longbox_postprocess::PendingInterventionsCache>,
) {
    let Some(raw) = config.download_watch_path.as_deref() else {
        info!(target: "longbox_web", "phase_b.disabled (DOWNLOAD_WATCH_PATH not set)");
        return;
    };
    let watch_path = std::path::PathBuf::from(crate::config::normalize_path(raw));
    let library_root = std::path::PathBuf::from(&config.library_root_path);
    // Poll interval from the settings table. get_or_default falls back
    // to 30 on a missing row or an unparseable value — defensive enough
    // that a typo in the row can't kill Phase B.
    let poll_interval_seconds: u64 = settings_repo::get_or_default(
        db,
        settings_repo::KEY_PHASE_B_POLL_INTERVAL_SECONDS,
        30u64,
    )
    .await
    .unwrap_or(30);
    let postprocess_config = longbox_postprocess::PostprocessConfig {
        watch_path: watch_path.clone(),
        library_root,
        poll_interval: Duration::from_secs(poll_interval_seconds),
    };
    match longbox_postprocess::start(postprocess_config, db.clone(), cache).await {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!(
                target: "longbox_web",
                err = %e,
                watch_path = %watch_path.display(),
                "phase_b.skipped_at_boot"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metron_disabled_yields_none_without_reading_credentials() {
        // Even with valid credentials present, kill switch off short-
        // circuits before any config touch. Pass deliberately-invalid
        // credentials to prove they're never used.
        let result = build_metron_client(false, Some("user"), Some("pass"));
        assert!(result.is_none(), "disabled kill switch must collapse to None");
    }

    #[test]
    fn metron_enabled_with_missing_user_degrades_to_none() {
        let result = build_metron_client(true, None, Some("pass"));
        assert!(
            result.is_none(),
            "enabled + missing user must degrade to None (not panic, not error)"
        );
    }

    #[test]
    fn metron_enabled_with_missing_password_degrades_to_none() {
        let result = build_metron_client(true, Some("user"), None);
        assert!(result.is_none());
    }

    #[test]
    fn metron_enabled_with_whitespace_credentials_degrades_to_none() {
        let result = build_metron_client(true, Some("   "), Some("   "));
        assert!(
            result.is_none(),
            "MetronClient::new rejects whitespace-only credentials; bootstrap collapses"
        );
    }

    #[test]
    fn metron_enabled_with_valid_credentials_returns_some() {
        let result = build_metron_client(true, Some("real_user"), Some("real_pass"));
        assert!(
            result.is_some(),
            "enabled + non-empty credentials must yield a constructed client handle"
        );
    }
}
