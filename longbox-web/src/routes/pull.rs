//! Pull engine — the manual "Check now" trigger and pull-list CRUD.
//!
//! The scheduled sweep and the engine itself live in `longbox-pull`;
//! `/pull/check` nudges the running scheduler. `/pull-list` manages
//! which series are subscribed for auto-pull. The list-view page and
//! the series-detail subscribe toggle are Phase A.8 Step 7.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{pull_list_repo, series_repo, NewPullEntry, PullListRow, PullListWithSeries};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/pull/check", post(check_now))
        .route("/pull-list", get(list).post(add))
        .route(
            "/pull-list/:series_id",
            get(get_one).patch(set_paused).delete(remove),
        )
}

/// Request an immediate pull sweep. `202 Accepted` when the request is
/// taken; `409 Conflict` when a sweep is already running — the engine
/// runs sweeps strictly one at a time.
async fn check_now(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    if state.pull.request_sweep() {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError::Conflict {
            code: "conflict.pull_running",
            message: "A pull sweep is already running.".into(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct AddBody {
    series_id: i64,
}

#[derive(Debug, Deserialize)]
struct PauseBody {
    paused: bool,
}

/// Every subscribed series, joined with its series fields — the
/// `/releases/pull-list` management view.
async fn list(State(state): State<AppState>) -> Result<Json<Vec<PullListWithSeries>>, ApiError> {
    Ok(Json(pull_list_repo::list_with_series(&state.db).await?))
}

/// The pull-list entry for one series, or `null` when not subscribed.
/// The series-detail page reads this to render the subscribe toggle.
async fn get_one(
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
) -> Result<Json<Option<PullListRow>>, ApiError> {
    Ok(Json(pull_list_repo::get(&state.db, series_id).await?))
}

/// Subscribe a series. `start_issue` defaults to null — pull from the
/// first solicited issue (no UI for the floor in Step 7).
async fn add(
    State(state): State<AppState>,
    Json(body): Json<AddBody>,
) -> Result<Json<PullListRow>, ApiError> {
    // Pre-flight the series so a bad id is a clean 404, not a raw
    // foreign-key violation surfacing from the INSERT.
    if series_repo::find_by_id(&state.db, body.series_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "series",
            id: body.series_id.to_string(),
        });
    }
    match pull_list_repo::add(
        &state.db,
        NewPullEntry {
            series_id: body.series_id,
            start_issue: None,
        },
    )
    .await
    {
        Ok(row) => Ok(Json(row)),
        Err(longbox_db::DbError::UniqueViolation {
            field: "pull_list_series_id",
        }) => Err(ApiError::Conflict {
            code: "conflict.already_on_pull_list",
            message: "That series is already on the pull list.".into(),
        }),
        Err(e) => Err(ApiError::from(e)),
    }
}

/// Pause or resume auto-pulls for a subscribed series.
async fn set_paused(
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
    Json(body): Json<PauseBody>,
) -> Result<Json<PullListRow>, ApiError> {
    pull_list_repo::set_paused(&state.db, series_id, body.paused)
        .await
        .map_err(|e| not_found_or(e, series_id))?;
    let row = pull_list_repo::get(&state.db, series_id)
        .await?
        .ok_or_else(|| not_found(series_id))?;
    Ok(Json(row))
}

/// Unsubscribe a series.
async fn remove(
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    pull_list_repo::remove(&state.db, series_id)
        .await
        .map_err(|e| not_found_or(e, series_id))?;
    Ok(StatusCode::NO_CONTENT)
}

fn not_found(series_id: i64) -> ApiError {
    ApiError::NotFound {
        resource: "pull_list entry",
        id: series_id.to_string(),
    }
}

fn not_found_or(e: longbox_db::DbError, series_id: i64) -> ApiError {
    match e {
        longbox_db::DbError::NotFound => not_found(series_id),
        other => ApiError::from(other),
    }
}
