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
