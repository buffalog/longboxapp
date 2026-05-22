use serde::{Deserialize, Serialize};
use time::PrimitiveDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanReport {
    pub library_root_id: i64,
    pub started_at: PrimitiveDateTime,
    pub completed_at: PrimitiveDateTime,
    pub duration_ms: u64,
    pub files_seen: u32,
    pub files_added: u32,
    pub files_updated: u32,
    pub files_marked_missing: u32,
    pub matched_owned: u32,
    pub matched_needs_review: u32,
    /// Preserved from prior status; the scanner never assigns `Ignored`.
    pub matched_ignored: u32,
    pub unmatched: u32,
    /// Series newly marked for automatic removal this scan (A.9 Step 6b).
    pub series_auto_marked: u32,
    /// Series hard-deleted this scan because their auto-tidy recovery
    /// window elapsed (A.9 Step 6b).
    pub series_auto_purged: u32,
    pub errors: Vec<ScanFileError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFileError {
    pub path_relative: String,
    pub error_message: String,
}

impl ScanReport {
    pub(crate) fn new(library_root_id: i64, started_at: PrimitiveDateTime) -> Self {
        Self {
            library_root_id,
            started_at,
            completed_at: started_at,
            duration_ms: 0,
            files_seen: 0,
            files_added: 0,
            files_updated: 0,
            files_marked_missing: 0,
            matched_owned: 0,
            matched_needs_review: 0,
            matched_ignored: 0,
            unmatched: 0,
            series_auto_marked: 0,
            series_auto_purged: 0,
            errors: Vec::new(),
        }
    }
}
