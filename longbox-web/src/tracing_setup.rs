use std::io::IsTerminal;

use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Initialize the global tracing subscriber. JSON output when stderr is not
/// a TTY (production / Docker / journald); pretty otherwise. Filter level
/// from `LOG_LEVEL`, falling back to `info`.
pub fn init(log_level: &str) {
    let filter = EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    let is_tty = std::io::stderr().is_terminal();
    let registry = tracing_subscriber::registry().with(filter);

    if is_tty {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(true)
                    .with_span_events(FmtSpan::NONE),
            )
            .init();
    } else {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(std::io::stderr)
                    .with_target(true)
                    .with_span_events(FmtSpan::NONE),
            )
            .init();
    }
}
