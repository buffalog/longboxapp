//! LongBox HTTP server. Composes longbox-{core,db,comicvine,scanner} behind
//! an Axum router.
//!
//! The library form (this module) exists so integration tests can spin up
//! the app in-process via [`build_router`] with a hand-rolled [`AppState`],
//! without going through `main.rs`'s env-var bootstrap.

pub mod bootstrap;
pub mod config;
pub mod content_hash;
pub mod error;
pub mod frontend;
pub mod metron_link;
pub(crate) mod pathsafe;
pub mod routes;
pub mod state;
pub mod tracing_setup;

pub use bootstrap::{run as bootstrap_run, BootstrapError};
pub use config::{AppConfig, ConfigError};
pub use error::{ApiError, FieldError};
pub use routes::{build_opds_router, build_router};
pub use state::{AppState, CurrentScan, ScanKind, ScanStatus, SubscribeOutcome, SubscribeStatus};
