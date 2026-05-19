//! Startup orchestration. Builds [`AppState`] from [`AppConfig`].

use std::sync::Arc;
use std::time::Duration;

use longbox_comicvine::{ComicVineClient, ComicVineClientConfig};
use longbox_db::{library_root_repo, NewLibraryRoot};
use longbox_scanner::{Scanner, ScannerConfig};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::info;

use crate::config::{normalize_path, AppConfig};
use crate::state::{AppState, ScanStatus};

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
    let scanner = Scanner::new(
        db.clone(),
        ScannerConfig {
            match_threshold: config.match_threshold,
        },
    );

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

    // 8. Compose state.
    Ok(AppState {
        db,
        cv: Arc::new(cv),
        scanner: Arc::new(scanner),
        config: Arc::new(config),
        scan_status: Arc::new(RwLock::new(ScanStatus::default())),
        library_root_id,
        pending_cache,
    })
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
    let postprocess_config = longbox_postprocess::PostprocessConfig {
        watch_path: watch_path.clone(),
        library_root,
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
