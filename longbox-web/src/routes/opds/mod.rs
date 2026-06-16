//! OPDS 1.2 catalog HTTP surface.
//!
//! The pure feed-construction and access-control logic lives in the
//! `longbox-opds` crate; this module holds the Axum handlers and the
//! per-request auth middleware. `build_router` mounts [`router`] under
//! `/opds`, so the routes here carry the `/v1` segment (a request to
//! `GET /opds/v1` matches the `/v1` route exactly, sidestepping axum's
//! nested-root trailing-slash gotcha).
//!
//! Every route is wrapped by [`require_auth`], which loads the four OPDS
//! `settings` rows on each request (so the enable toggle and credential
//! changes take effect with no restart) and maps the result to:
//!   - `503` — OPDS disabled or no username configured,
//!   - `401` + `WWW-Authenticate: Basic realm="LongBox"` — missing/bad creds,
//!   - passthrough — Basic or Bearer credentials accepted.

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use longbox_db::settings_repo;
use longbox_opds::{AuthOutcome, OpdsAccess};

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

/// Per-request OPDS auth gate. Reads the four OPDS settings rows, then
/// delegates the actual decision to the pure [`OpdsAccess::authorize`].
async fn require_auth(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let access = match load_access(&state).await {
        Ok(access) => access,
        Err(err) => {
            tracing::error!(error = %err, "failed to load OPDS access settings");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let authorization = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    match access.authorize(authorization) {
        AuthOutcome::Authorized => next.run(req).await,
        AuthOutcome::Unconfigured => (
            StatusCode::SERVICE_UNAVAILABLE,
            "OPDS access is not configured. Enable OPDS and set a username in LongBox Settings.",
        )
            .into_response(),
        AuthOutcome::Unauthorized => unauthorized(),
    }
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

/// Load the OPDS access settings into the pure [`OpdsAccess`] verifier.
/// Empty stored values (the migration seeds username/hash/token as `''`)
/// collapse to `None` so the verifier treats them as absent.
async fn load_access(state: &AppState) -> Result<OpdsAccess, longbox_db::DbError> {
    let enabled: bool =
        settings_repo::get_or_default(&state.db, settings_repo::KEY_OPDS_ENABLED, false).await?;
    let username = non_empty(settings_repo::get(&state.db, settings_repo::KEY_OPDS_USERNAME).await?);
    let password_hash =
        non_empty(settings_repo::get(&state.db, settings_repo::KEY_OPDS_PASSWORD_HASH).await?);
    let api_token =
        non_empty(settings_repo::get(&state.db, settings_repo::KEY_OPDS_API_TOKEN).await?);

    Ok(OpdsAccess {
        enabled,
        username,
        password_hash,
        api_token,
    })
}

/// `Some(value)` only when it has non-whitespace content; otherwise `None`.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}
