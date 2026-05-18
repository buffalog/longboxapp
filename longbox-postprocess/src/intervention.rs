use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A file that Phase B couldn't process automatically and is waiting on
/// the user. Lives in an in-memory cache shared with `longbox-web` (the
/// shared `Arc<RwLock<Vec<…>>>` lands in Step 7, when the conflict /
/// failure surface is wired into the dashboard).
///
/// Not persisted to a table for v1 — Phase B's "pending" set is
/// rebuilt from filesystem state at every startup; durability beyond
/// the process lifetime is on the Phase B+ cleanup queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingIntervention {
    /// Where the file currently sits (under the watch folder).
    pub source_path: PathBuf,
    /// Where Phase B *would* have moved it absent the failure.
    pub target_path: PathBuf,
    pub reason: InterventionReason,
    pub size: i64,
    #[serde(with = "time::serde::iso8601")]
    pub last_attempt: OffsetDateTime,
}

/// Why a file is sitting in `_pending` rather than the library proper.
/// The string payloads on the non-Conflict variants carry the underlying
/// error message verbatim — useful for the dashboard's reason column
/// when the user is deciding what to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum InterventionReason {
    /// Target library path already exists; Phase B refuses to overwrite.
    Conflict,
    /// The CBZ was matched but Phase B couldn't write the ComicInfo
    /// payload into it (zip-write failure, disk full, permission error).
    ComicInfoWriteFailed(String),
    /// File move from watch folder to final library path failed
    /// (cross-device error, target permissions, race condition).
    MoveFailed(String),
}
