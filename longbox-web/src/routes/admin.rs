//! Admin/operations endpoints. Currently a single restart action used
//! by the Settings page's "Restart LongBox" button. The handler relies
//! on Docker's `restart: unless-stopped` policy: respond 202, sleep
//! briefly so the response can ship, then exit the process — Docker
//! brings the container back up.

use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/admin/restart", post(restart))
}

async fn restart() -> StatusCode {
    tokio::spawn(async {
        // Brief delay so the 202 response actually reaches the client
        // before we tear the process down.
        tokio::time::sleep(Duration::from_millis(500)).await;
        tracing::warn!(target: "longbox_web", "admin restart requested — exiting");
        std::process::exit(0);
    });
    StatusCode::ACCEPTED
}
