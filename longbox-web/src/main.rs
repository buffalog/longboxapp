use std::process::ExitCode;

use longbox_db::scan_run_repo;
use longbox_web::{bootstrap_run, build_router, config::AppConfig, tracing_setup};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> ExitCode {
    // Load config from env before initializing tracing so we have the
    // requested log level.
    let config = match AppConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("longbox: configuration error: {e}");
            return ExitCode::from(1);
        }
    };

    tracing_setup::init(&config.log_level);
    info!(
        target: "longbox_web",
        version = env!("CARGO_PKG_VERSION"),
        bind = %config.bind_addr,
        db = %config.database_url,
        library_root = %config.library_root_path,
        "starting longbox"
    );

    let state = match bootstrap_run(config.clone()).await {
        Ok(s) => s,
        Err(e) => {
            error!(target: "longbox_web", err = %e, "bootstrap failed");
            eprintln!("longbox: bootstrap failed: {e}");
            return ExitCode::from(1);
        }
    };

    // Task C startup sweep: any scan_runs row left in `running` belongs
    // to a previous process that didn't get to update it on the way out
    // (crash, kill, container restart). Mark them failed so /scans
    // reflects reality instead of pretending a scan is still in flight.
    // Best-effort — log but don't fail boot.
    match scan_run_repo::mark_running_as_failed(&state.db, "interrupted by restart").await {
        Ok(0) => {}
        Ok(n) => info!(
            target: "longbox_web",
            count = n,
            "marked stale scan_runs rows as failed"
        ),
        Err(e) => warn!(
            target: "longbox_web",
            err = %e,
            "scan_runs startup sweep failed; continuing"
        ),
    }

    let bind = state.config.bind_addr.clone();
    let listener = match TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            error!(target: "longbox_web", err = %e, "bind failed");
            eprintln!("longbox: bind to {bind} failed: {e}");
            return ExitCode::from(1);
        }
    };
    info!(target: "longbox_web", addr = %bind, "listening");

    let router = build_router(state);
    if let Err(e) = axum::serve(listener, router).await {
        error!(target: "longbox_web", err = %e, "server exited with error");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
