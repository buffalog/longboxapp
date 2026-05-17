use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use longbox_db::{library_root_repo, LibraryRootRow};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/library-roots", get(list))
}

/// Returns the configured library roots. Phase A always returns exactly one
/// element (bootstrap upserts a single row from `LIBRARY_ROOT_PATH`), but
/// the array shape future-proofs for multi-root in Phase B without
/// frontend churn.
async fn list(State(state): State<AppState>) -> Result<Json<Vec<LibraryRootRow>>, ApiError> {
    Ok(Json(library_root_repo::list_all(&state.db).await?))
}
