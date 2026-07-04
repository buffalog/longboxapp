//! Route assembly. Each handler module exposes a `router()` function that
//! returns a `Router<AppState>`; this module composes them under `/api` and
//! attaches workspace-wide middleware (trace, CORS).

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub mod admin;
pub mod calendar;
pub mod creators;
pub mod cv_search;
pub mod dashboard;
pub mod downloader;
pub mod enrichment;
pub mod files;
pub mod health;
pub mod indexers;
pub mod interactive_search;
pub mod library_roots;
pub mod missing;
pub mod needs_attention;
pub mod opds;
pub mod opds_settings;
pub mod opds_users;
pub mod postprocess;
pub mod publishers;
pub mod pull;
pub mod reader;
pub mod reconcile;
pub mod scan;
pub mod series;
pub mod settings;
pub mod stats;
pub mod webhooks;

pub fn build_router(state: AppState) -> Router {
    let cors = if state.config.cors_permissive {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        CorsLayer::new()
    };

    let api = Router::new()
        .merge(health::router())
        .merge(creators::router())
        .merge(cv_search::router())
        .merge(series::router())
        .merge(files::router())
        .merge(scan::router())
        .merge(stats::router())
        .merge(library_roots::router())
        .merge(settings::router())
        .merge(opds_settings::router())
        .merge(opds_users::router())
        .merge(dashboard::router())
        .merge(missing::router())
        .merge(postprocess::router())
        .merge(publishers::router())
        .merge(indexers::router())
        .merge(interactive_search::router())
        .merge(downloader::router())
        .merge(webhooks::router())
        .merge(pull::router())
        .merge(reader::router())
        .merge(reconcile::router())
        .merge(calendar::router())
        .merge(needs_attention::router())
        .merge(enrichment::router())
        .merge(admin::router());

    Router::new()
        .nest("/api", api)
        .nest("/opds", opds::router(state.clone()))
        .fallback(crate::frontend::fallback_handler)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

/// OPDS-only router for the dedicated listener (port 8096). Serves
/// `/opds/v1/*` (auth-gated, same middleware as the main app) and nothing
/// else — the default fallback 404s every other path, so this surface
/// exposes no admin API. Shares the same [`AppState`] as the main app.
pub fn build_opds_router(state: AppState) -> Router {
    Router::new()
        .nest("/opds", opds::router(state.clone()))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
