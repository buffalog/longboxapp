use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::AppConfig;

const RECENT_SCAN_CAP: usize = 10;

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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScanStatus {
    pub current: Option<CurrentScan>,
    /// Last 10 completed scan reports, newest first.
    pub recent: VecDeque<longbox_scanner::ScanReport>,
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

impl ScanStatus {
    /// Push a completed scan onto the recent ring, truncating to the cap.
    pub fn record(&mut self, report: longbox_scanner::ScanReport) {
        self.recent.push_front(report);
        while self.recent.len() > RECENT_SCAN_CAP {
            self.recent.pop_back();
        }
    }
}
