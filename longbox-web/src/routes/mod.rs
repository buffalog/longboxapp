//! Route assembly. Each handler module exposes a `router()` function that
//! returns a `Router<AppState>`; this module composes them under `/api` and
//! attaches workspace-wide middleware (trace, CORS).

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub mod cv_search;
pub mod files;
pub mod health;
pub mod library_roots;
pub mod scan;
pub mod series;
pub mod stats;

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
        .merge(library_roots::router());

    Router::new()
        .nest("/api", api)
        .fallback(crate::frontend::fallback_handler)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}
