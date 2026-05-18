use std::path::PathBuf;

/// Runtime configuration for the Phase B watcher. Composed by
/// `longbox-web` at startup from `DOWNLOAD_WATCH_PATH` (sets
/// [`watch_path`](Self::watch_path)) and the catalog's configured
/// library root (sets [`library_root`](Self::library_root)).
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
    /// Absolute path to the catalog's library root. Imports land in
    /// the per-series subfolder of this directory; the `_unsorted/`
    /// fallback also lives under it.
    pub library_root: PathBuf,
}
