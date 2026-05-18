//! Phase B post-process pipeline: filesystem watcher + per-file
//! processing for new arrivals in the download folder.
//!
//! Step 4 ships the crate skeleton — public types and the [`start`]
//! entry point — but contains no watcher or processing logic. Step 5
//! adds the `notify`-based watcher + initial sweep; Step 6 wires the
//! matcher, ComicInfo writer, library convention, file move, and
//! catalog insert. Until then, [`start`] is a no-op stub that logs and
//! returns.
//!
//! Architecture per `longbox-phase-b-prompt.md`:
//! - Hard crate boundary: `longbox-postprocess` owns its watcher and
//!   processing pipeline; `longbox-web` calls only [`start`].
//! - `longbox-comicvine` is not a dependency — Phase B never talks to
//!   CV. All metadata comes from the catalog.
//! - `longbox-scanner` is not a dependency — Phase B is event-driven,
//!   not walk-driven; the matcher lives in `longbox-core`.

pub mod config;
pub mod error;
pub mod intervention;

pub use config::PostprocessConfig;
pub use error::{PostprocessError, Result};
pub use intervention::{InterventionReason, PendingIntervention};

/// Entry point called from `longbox-web` at startup when
/// `DOWNLOAD_WATCH_PATH` is configured and readable.
///
/// **Step 4 skeleton:** logs a structured `phase_b.started` line and
/// returns immediately. No filesystem watcher, no processing. Step 5
/// turns this into the long-running `notify` loop that owns the
/// watcher; Step 6 plumbs the per-file processing pipeline through it.
///
/// The signature is the final public shape — later steps grow the
/// body, not the parameter list. `db` is passed as
/// [`longbox_db::Pool`] (a [`sqlx::SqlitePool`] alias) because the
/// processing pipeline needs both reads (matcher → series + issue
/// lookup) and writes ([`longbox_db::file_repo::upsert_imported`]).
pub async fn start(config: PostprocessConfig, _db: longbox_db::Pool) -> Result<()> {
    tracing::info!(
        target: "longbox_postprocess",
        watch_path = %config.watch_path.display(),
        library_root = %config.library_root.display(),
        "phase_b.started (skeleton — no watcher wired yet)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn start_skeleton_returns_ok() {
        // The skeleton entry point is a no-op log; calling it should
        // never fail. Step 5 will introduce real failure modes (bad
        // watch path, etc.) and replace this test with one that
        // exercises them.
        let pool = longbox_db::open(":memory:").await.unwrap();
        let config = PostprocessConfig {
            watch_path: PathBuf::from("/tmp/longbox-test-watch"),
            library_root: PathBuf::from("/tmp/longbox-test-library"),
        };
        start(config, pool).await.unwrap();
    }
}
