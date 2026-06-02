//! LongBox pull engine — the scheduled auto-download workflow.
//!
//! Subscribed series (the `pull_list`) are swept on a daily schedule:
//! for each, newly-shipped un-owned issues are searched across the
//! configured Newznab indexers and, on a hit, their NZB is handed to
//! the configured downloader. Submitted downloads are tracked in
//! `pull_attempts`; Phase B catches the completed file in the watch
//! folder and attributes it to the pull engine.
//!
//! Boot shape mirrors `longbox_postprocess::start`: [`start`] spawns a
//! supervised scheduler task and returns a [`PullHandle`] the web layer
//! parks in its shared state for the manual "Check now" trigger.
//!
//! Crate boundary — like Phase B, the pull engine never talks to
//! ComicVine. It acts only on issues already in the catalog; see
//! [`engine`] for the consequence and the intended discovery path.

mod config;
mod dispatch;
mod engine;
mod error;
mod schedule;
mod search;

pub use config::PullConfig;
pub use engine::{sweep, sweep_single_issue, sweep_single_series, SweepSummary};
pub use error::PullError;
pub use schedule::PullHandle;
pub use search::{fire_issue_search, PullSearchHandle};

/// Start the pull engine: spawn the daily scheduler task and return a
/// handle for manual sweep triggering. Called once at web-layer boot;
/// the scheduler task runs for the lifetime of the process.
pub fn start(config: PullConfig, db: longbox_db::Pool) -> PullHandle {
    schedule::spawn(config, db)
}
