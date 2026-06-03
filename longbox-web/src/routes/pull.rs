//! Pull engine — the manual "Check now" trigger and pull-list CRUD.
//!
//! The scheduled sweep and the engine itself live in `longbox-pull`;
//! `/pull/check` nudges the running scheduler. `/pull-list` manages
//! which series are subscribed for auto-pull. The list-view page and
//! the series-detail subscribe toggle are Phase A.8 Step 7.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{
    issue_repo, pull_list_repo, series_repo, NewPullEntry, PullListRow, PullListWithSeries,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/pull/check", post(check_now))
        .route("/pull/search/:series_id", post(search_now))
        .route(
            "/pull/search/:series_id/issue/:issue_id",
            post(search_issue_now),
        )
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
            details: serde_json::Value::Null,
        })
    }
}

#[derive(Debug, Serialize)]
struct SearchNowResponse {
    queued: usize,
    note: Option<String>,
}

/// Request an on-demand "search every missing issue" for a series.
/// `202 Accepted` when at least one issue was queued, with
/// `{queued: N}` carrying the count. `200 OK` with `{queued: 0,
/// note: ...}` when the series has no missing issues — there's
/// nothing to fail, but the caller deserves to know the dispatch
/// was a no-op. `404 Not Found` when the series id doesn't exist.
///
/// The series does NOT need to be on the pull list — this is the
/// header "Search missing" affordance on series detail, mirroring
/// the per-issue Search button on each missing row. The
/// pull-list-based per-series guard
/// (`state.pull_search.try_start`) is bypassed too: it feeds the
/// daily sweep's all-series cycle, which is the wrong semantics
/// for a manual user override on a possibly-unsubscribed series.
/// Per-issue in-flight dedup is the engine's job — `sweep_single_issue`
/// silently no-ops on already-in-flight issues, so this loop is
/// safe to call repeatedly.
async fn search_now(
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
) -> Result<Response, ApiError> {
    // 404 preflight — clean structured error rather than a
    // misleading empty-results path. Same shape as
    // `search_issue_now`.
    if series_repo::find_by_id(&state.db, series_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "series",
            id: series_id.to_string(),
        });
    }
    let missing = issue_repo::list_missing_for_series(&state.db, series_id).await?;
    if missing.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(SearchNowResponse {
                queued: 0,
                note: Some("No missing issues for this series.".into()),
            }),
        )
            .into_response());
    }
    let queued = missing.len();
    for issue in missing {
        longbox_pull::fire_issue_search(state.db.clone(), series_id, issue.id);
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(SearchNowResponse { queued, note: None }),
    )
        .into_response())
}

/// Request an on-demand search for ONE specific issue. The series
/// does NOT need to be on the pull list — this is the per-issue
/// "Search" button on the series detail page for Missing/Unowned
/// issues. `202 Accepted` always when the (series, issue, relation)
/// preflight passes; `404 Not Found` for an unknown series, unknown
/// issue, or an issue that doesn't belong to the named series. No
/// 409 — the in-flight guard lives inside the engine
/// ([`longbox_pull::sweep_single_issue`]) and silently skips, since
/// the user-visible contract for the button is "fire and forget."
async fn search_issue_now(
    State(state): State<AppState>,
    Path((series_id, issue_id)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    // Series preflight — clean 404 here rather than letting the
    // engine's typed error round-trip.
    if series_repo::find_by_id(&state.db, series_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "series",
            id: series_id.to_string(),
        });
    }
    // Issue preflight — same shape. Also enforces the relation so a
    // tampered URL (issue exists, but for a different series) 404s
    // instead of triggering a search the user didn't intend.
    let issue = issue_repo::find_by_id(&state.db, issue_id)
        .await?
        .ok_or(ApiError::NotFound {
            resource: "issue",
            id: issue_id.to_string(),
        })?;
    if issue.series_id != series_id {
        return Err(ApiError::NotFound {
            resource: "issue",
            id: issue_id.to_string(),
        });
    }
    longbox_pull::fire_issue_search(state.db.clone(), series_id, issue_id);
    Ok(StatusCode::ACCEPTED)
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
        Ok(row) => {
            // Auto-fire an on-demand search for the newly-subscribed
            // series. Fire-and-forget: a duplicate (someone manually
            // clicked Search now in the same window) is a silent
            // no-op via the per-series guard. No toast — the user
            // sees results land on the pull list / needs-attention
            // page naturally.
            state.pull_search.try_start(row.series_id).await;
            Ok(Json(row))
        }
        Err(longbox_db::DbError::UniqueViolation {
            field: "pull_list_series_id",
        }) => Err(ApiError::Conflict {
            code: "conflict.already_on_pull_list",
            message: "That series is already on the pull list.".into(),
            details: serde_json::Value::Null,
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
