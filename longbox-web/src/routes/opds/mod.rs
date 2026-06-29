//! OPDS 1.2 catalog HTTP surface.
//!
//! The pure feed-construction and access-control logic lives in the
//! `longbox-opds` crate; this module holds the Axum handlers and the
//! per-request auth middleware. `build_router` mounts [`router`] under
//! `/opds`, so the routes here carry the `/v1` segment (a request to
//! `GET /opds/v1` matches the `/v1` route exactly, sidestepping axum's
//! nested-root trailing-slash gotcha).
//!
//! Every route is wrapped by [`require_auth`], which on each request checks
//! the global `opds_enabled` toggle and authenticates HTTP Basic credentials
//! against the per-user `opds_users` table (so the toggle and account changes
//! take effect with no restart):
//!   - `503` — OPDS disabled,
//!   - `401` + `WWW-Authenticate: Basic realm="LongBox"` — missing/bad creds,
//!   - passthrough — a valid enabled account's Basic credentials.

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use longbox_db::{opds_users_repo, settings_repo};

use crate::state::AppState;

mod covers;
mod download;
mod feeds;

/// Build the OPDS sub-router. Mounted at `/opds` by `build_router`; every
/// route is guarded by [`require_auth`]. The state is baked into the auth
/// middleware via `from_fn_with_state` (the route handlers receive it the
/// usual way once `build_router` calls `.with_state`).
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/v1", get(feeds::root))
        .route("/v1/series", get(feeds::series_list))
        .route("/v1/series/:id", get(feeds::series_detail))
        .route("/v1/publishers", get(feeds::publishers_list))
        .route("/v1/publishers/:name/series", get(feeds::publisher_series))
        .route("/v1/search", get(feeds::search))
        .route("/v1/opensearch.xml", get(feeds::opensearch))
        .route("/v1/covers/:issue_id", get(covers::cover))
        .route("/v1/issues/:id/download", get(download::download))
        // INVARIANT: every OPDS route MUST be registered ABOVE this
        // `.layer()` call. In axum a layer only wraps the routes that
        // already exist when it is applied — any `.route()` chained AFTER
        // this line would be UNAUTHENTICATED. Later commits add feed,
        // cover, and download routes: add them above, never below.
        .layer(middleware::from_fn_with_state(state, require_auth))
}

/// Per-request OPDS auth gate. Checks the global `opds_enabled` toggle, then
/// authenticates HTTP Basic credentials against the `opds_users` table.
async fn require_auth(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    // 1. Global toggle. Off → 503 regardless of credentials.
    match settings_repo::get_or_default(&state.db, settings_repo::KEY_OPDS_ENABLED, false).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "OPDS access is disabled. Enable it in LongBox Settings.",
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!(error = %err, "failed to read opds_enabled");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // 2. Parse HTTP Basic credentials (the only accepted scheme).
    let Some((username, password)) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(longbox_opds::parse_basic)
    else {
        return unauthorized();
    };

    // 3. Look up the enabled account, verify bcrypt. On the not-found path,
    //    burn an equivalent bcrypt cycle so timing doesn't leak which
    //    usernames exist.
    let user = match opds_users_repo::find_enabled_for_auth(&state.db, &username).await {
        Ok(user) => user,
        Err(err) => {
            tracing::error!(error = %err, "failed to look up OPDS user");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let authorized = match &user {
        Some(u) => longbox_opds::verify_password(&password, &u.password_hash),
        None => longbox_opds::dummy_verify(&password),
    };
    if !authorized {
        return unauthorized();
    }

    // 4. Success — stamp last_seen (best-effort; a failure here must not
    //    sink an otherwise-valid request) and pass through.
    if let Some(u) = user {
        if let Err(err) = opds_users_repo::touch_last_seen(&state.db, u.id).await {
            tracing::warn!(error = %err, user_id = u.id, "failed to update OPDS last_seen_at");
        }
    }
    next.run(req).await
}

/// `401` with the Basic-auth challenge OPDS readers expect.
fn unauthorized() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"LongBox\""),
    );
    response
}
