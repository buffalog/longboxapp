//! Route assembly. Each handler module exposes a `router()` function that
//! returns a `Router<AppState>`; this module composes them under `/api` and
//! attaches workspace-wide middleware (trace, CORS).

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub mod calendar;
pub mod cv_search;
pub mod dashboard;
pub mod downloader;
pub mod enrichment;
pub mod files;
pub mod health;
pub mod indexers;
pub mod library_roots;
pub mod missing;
pub mod needs_attention;
pub mod postprocess;
pub mod publishers;
pub mod pull;
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
        .merge(cv_search::router())
        .merge(series::router())
        .merge(files::router())
        .merge(scan::router())
        .merge(stats::router())
        .merge(library_roots::router())
        .merge(settings::router())
        .merge(dashboard::router())
        .merge(missing::router())
        .merge(postprocess::router())
        .merge(publishers::router())
        .merge(indexers::router())
        .merge(downloader::router())
        .merge(webhooks::router())
        .merge(pull::router())
        .merge(reconcile::router())
        .merge(calendar::router())
        .merge(needs_attention::router())
        .merge(enrichment::router());

    Router::new()
        .nest("/api", api)
        .fallback(crate::frontend::fallback_handler)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}
