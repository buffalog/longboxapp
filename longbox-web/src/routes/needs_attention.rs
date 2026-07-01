//! The needs-attention surface — pull-side failures.
//!
//! `GET /api/needs-attention/pull-failures` lists issues whose most
//! recent pull attempt failed; `POST /api/needs-attention/retry`
//! un-parks an issue (clears its failed attempts) and nudges a sweep.
//! Phase B's intervention failures keep their own endpoint
//! (`/api/postprocess/pending`) — the `/needs-attention` page reads
//! both and renders them as two sections.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use longbox_db::{pull_attempt_repo, FailedPull};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/needs-attention/pull-failures", get(pull_failures))
        .route(
            "/needs-attention/pull-failures",
            delete(dismiss_all_pull_failures),
        )
        .route(
            "/needs-attention/pull-failures/:attempt_id",
            delete(dismiss_pull_failure),
        )
        .route("/needs-attention/retry", post(retry))
}

/// A pull failure plus its category. `category` is derived from
/// `status` + `release_id`: `'mismatched'` → `series_mismatch` (Bug 3 —
/// no release survived the pull engine's pre-grab series-title filter);
/// `'failed'` + no `release_id` → `submission_failed` (the downloader
/// never accepted the NZB); `'failed'` + `release_id` → `grab_failed`
/// (the download itself errored).
#[derive(Debug, Serialize)]
struct PullFailureRow {
    #[serde(flatten)]
    failure: FailedPull,
    category: &'static str,
}

#[derive(Debug, Deserialize)]
struct RetryBody {
    series_id: i64,
    issue_id: i64,
}

async fn pull_failures(
    State(state): State<AppState>,
) -> Result<Json<Vec<PullFailureRow>>, ApiError> {
    let rows = pull_attempt_repo::list_failed(&state.db)
        .await?
        .into_iter()
        .map(|failure| {
            let category = match failure.status.as_str() {
                "mismatched" => "series_mismatch",
                _ if failure.release_id.is_none() => "submission_failed",
                _ => "grab_failed",
            };
            PullFailureRow { failure, category }
        })
        .collect();
    Ok(Json(rows))
}

/// Un-park an issue: clear its `failed` pull attempts so the next sweep
/// retries it, then nudge an immediate sweep. Idempotent — a stale UI
/// retrying an already-cleared issue is a clean no-op.
async fn retry(
    State(state): State<AppState>,
    Json(body): Json<RetryBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cleared =
        pull_attempt_repo::clear_failed_for_issue(&state.db, body.series_id, body.issue_id).await?;
    // Best-effort — if a sweep is already running, the un-parked issue
    // is picked up by the next one.
    state.pull.request_sweep();
    Ok(Json(serde_json::json!({ "cleared": cleared })))
}

/// Dismiss a single pull-failure surface row by `pull_attempts.id`.
/// Surgical on the failure-class rows — does not touch any older
/// failure-class attempt for the same issue, so a deeper history row
/// may resurface on the next list. 204 on both delete and no-op
/// (stale UI dismissing an already-gone row).
///
/// Also purges any stale `submitted` row (>
/// `STALE_SUBMITTED_HOURS` old) AND any stale `grabbed` row
/// (> `STALE_GRABBED_HOURS` old without a backing owned file) for
/// the same issue at the same time. Without this, the user's
/// observed workflow is:
///   1. Dismiss the visible failure on Needs-Attention.
///   2. Click Search to retry.
///   3. Nothing happens — the issue still has an orphaned stale
///      `submitted` row from a prior run (poll loop never resolved
///      it; SAB evicted the job) OR a stuck `grabbed` row whose
///      Phase B import never landed.
///      The dismiss action covers all three rows in one trip so future
///      searches actually fire.
async fn dismiss_pull_failure(
    State(state): State<AppState>,
    Path(attempt_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    // Look up (series_id, issue_id) BEFORE the delete so we can
    // chase the stale-submitted purge for the same issue. None when
    // the row already vanished (double-click / stale UI).
    let owner = pull_attempt_repo::find_series_issue_by_id(&state.db, attempt_id).await?;
    pull_attempt_repo::delete_by_id(&state.db, attempt_id).await?;
    if let Some((series_id, issue_id)) = owner {
        let purged = pull_attempt_repo::purge_stale_submitted_for_issue(
            &state.db, series_id, issue_id,
        )
        .await?;
        if purged > 0 {
            tracing::info!(
                target: "longbox_web",
                series_id,
                issue_id,
                purged,
                "dismiss_pull_failure.stale_submitted_purged"
            );
        }
        let purged_grabbed = pull_attempt_repo::purge_stale_grabbed_for_issue(
            &state.db, series_id, issue_id,
        )
        .await?;
        if purged_grabbed > 0 {
            tracing::info!(
                target: "longbox_web",
                series_id,
                issue_id,
                purged = purged_grabbed,
                "dismiss_pull_failure.stale_grabbed_purged"
            );
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Bulk-dismiss every failure-class `pull_attempts` row — the "Clear
/// all" action. Mirrors `list_failed`'s status filter
/// (`'failed'`, `'mismatched'`), so a `pending` / `submitted` / `grabbed`
/// attempt history is preserved. 204 even if zero rows matched.
async fn dismiss_all_pull_failures(
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    pull_attempt_repo::delete_all_failed(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
