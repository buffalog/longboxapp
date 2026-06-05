use std::path::PathBuf;
use std::time::Duration;

/// Runtime configuration for the Phase B watcher. Composed by
/// `longbox-web` at startup from `DOWNLOAD_WATCH_PATH` (sets
/// [`watch_path`](Self::watch_path)), the catalog's configured library
/// root (sets [`library_root`](Self::library_root)), and the
/// `phase_b_poll_interval_seconds` settings row (sets
/// [`poll_interval`](Self::poll_interval)).
///
/// Phase B is enabled implicitly: if `DOWNLOAD_WATCH_PATH` is set and
/// points to a readable directory, the web layer constructs this struct
/// and hands it to [`crate::start`]. Unset or unreadable → no
/// `PostprocessConfig` constructed, [`crate::start`] is never called,
/// and Phase B simply doesn't run.
#[derive(Debug, Clone)]
pub struct PostprocessConfig {
    /// Absolute path to the watch folder. CBZs that land in here (or
    /// any subdirectory) are candidates for import.
    pub watch_path: PathBuf,
    /// Absolute path to the catalog's library root. Owned imports land
    /// in the per-series subfolder of this directory. Unplaceable
    /// files stay in `watch_path` (no `_unsorted/` fallback per
    /// Jeremy's directive — the watch folder is the holding pen).
    pub library_root: PathBuf,
    /// How often the polling watcher walks `watch_path`. From the
    /// `phase_b_poll_interval_seconds` settings row at boot, default
    /// 30s. inotify is blind to host writes through Docker Desktop's
    /// virtiofs mount, so the watcher polls instead — see
    /// `lib::spawn_watcher` for the inotify→poll swap and revert path.
    pub poll_interval: Duration,
}
