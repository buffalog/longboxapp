//! Pull engine configuration.

use time::Time;

/// Pull engine configuration — built by the web layer from env vars.
#[derive(Debug, Clone)]
pub struct PullConfig {
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
        }
    }
}
