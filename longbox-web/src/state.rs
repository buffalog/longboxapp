use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::AppConfig;

#[derive(Clone)]
pub struct AppState {
    pub db: longbox_db::Pool,
    pub cv: Arc<longbox_comicvine::ComicVineClient>,
    pub scanner: Arc<longbox_scanner::Scanner>,
    pub config: Arc<AppConfig>,
    pub scan_status: Arc<RwLock<ScanStatus>>,
    /// The library root row id created or matched at bootstrap. Phase A
    /// has exactly one; cached here so handlers don't re-query.
    pub library_root_id: i64,
    /// Phase B's pending-intervention cache. Read by the dashboard
    /// counter and the `/files/pending-intervention` list view; written
    /// by the postprocess consumer task and the notify watcher. Always
    /// present (empty when Phase B is disabled or no files are stuck)
    /// so handlers don't need to special-case the absent state.
    pub pending_cache: Arc<longbox_postprocess::PendingInterventionsCache>,
    /// Handle to the Phase A.8 pull engine. Always present — the
    /// scheduler task runs unconditionally; the `/pull/check` route
    /// uses this to trigger an immediate sweep.
    pub pull: longbox_pull::PullHandle,
    /// Handle to the Library Tidy scheduled-scan timer. Always present;
    /// the daily scan runs through the same `scan_status` guard as the
    /// manual scan route.
    pub scan_scheduler: longbox_scan_scheduler::ScanSchedulerHandle,
    /// Handle to the A.9 Step 6c.2 CV enrichment worker. Always present;
    /// the worker enforces its own startup migration check and refuses
    /// to attempt anything if the 6c.1 columns are missing, so this
    /// handle is safe to expose regardless of schema state.
    pub enrichment: longbox_cv_enrichment::EnrichmentHandle,
}

/// In-memory mid-scan status. The "current" pill in the UI reads from
/// this; persisted scan history lives in the `scan_runs` table (read via
/// `scan_run_repo::list_recent`). Per Task C: in-memory state is only for
/// the live in-flight indicator, never for history rendering.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScanStatus {
    pub current: Option<CurrentScan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentScan {
    pub scan_id: String,
    pub library_root_id: i64,
    pub kind: ScanKind,
    #[serde(with = "time::serde::iso8601")]
    pub started_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanKind {
    Full,
    RescanUnmatched,
    RematchForSeries,
}
