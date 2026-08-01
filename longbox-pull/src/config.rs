//! Pull engine configuration.

use std::path::PathBuf;

use time::Time;

/// Pull engine configuration — built by the web layer from env vars.
#[derive(Debug, Clone)]
pub struct PullConfig {
    /// Phase B's watch folder, when one is configured
    /// (`DOWNLOAD_WATCH_PATH`). The sweep resolves a completed
    /// download's output folder against this to decide whether
    /// anything importable actually landed — see `crate::landed`.
    ///
    /// `None` (Phase B disabled) turns that check off and the engine
    /// behaves as it did before: a download that contained no comic
    /// ages out as "lost track of download". Supplied by the web layer
    /// rather than read from the environment here so both crates
    /// normalize the path exactly one way.
    pub watch_root: Option<PathBuf>,
    /// Wall-clock time the daily sweep fires.
    ///
    /// Interpreted as **UTC**, not server-local time:
    /// `time::OffsetDateTime::now_local()` is unreliable inside a
    /// multi-threaded Tokio process (it returns `IndeterminateOffset`
    /// rather than risk an unsound read of the TZ environment), and the
    /// workarounds are disproportionate for a once-a-day sweep. The web
    /// layer documents `PULL_SCHEDULE_TIME` as UTC.
    pub daily_time: Time,
}

impl Default for PullConfig {
    fn default() -> Self {
        Self {
            daily_time: time::macros::time!(05:00),
            watch_root: None,
        }
    }
}
