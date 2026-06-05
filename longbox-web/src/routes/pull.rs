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
use time::macros::format_description;

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
        .route("/pull/search-all-missing", post(search_all_missing))
        .route("/pull-list", get(list).post(add))
        .route("/pull-list/export", get(export_pull_list))
        .route("/pull-list/import", post(import_pull_list))
        .route(
            "/pull-list/:series_id",
            get(get_one).patch(set_paused).delete(remove),
        )
}

/// Rust port of the frontend `isSolicited` predicate. A "solicited"
/// issue is one with a fully-specified `YYYY-MM-DD` cover_date on or
/// after `today` — it hasn't shipped yet, so the pull engine has nothing
/// to find. Anything that isn't a clean `YYYY-MM-DD` (null, partial,
/// garbage) is treated as NOT solicited — same lenient classification
/// as the frontend, so a back-end skip on Search-all and a front-end
/// "this looks searchable" badge can't diverge.
fn is_solicited(cover_date: Option<&str>, today: time::Date) -> bool {
    let Some(raw) = cover_date else { return false };
    if raw.len() != 10 {
        return false;
    }
    let fmt = format_description!("[year]-[month]-[day]");
    match time::Date::parse(raw, fmt) {
        Ok(d) => d >= today,
        Err(_) => false,
    }
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

#[derive(Debug, Serialize)]
struct SearchAllMissingResponse {
    /// Issues for which a per-issue search was dispatched.
    searched: usize,
    /// Issues whose cover_date is today or later — skipped because the
    /// release hasn't happened yet, so the indexer call would be pure
    /// noise. Matches the frontend's `isSolicited` predicate exactly.
    skipped_solicited: usize,
}

/// Dispatch a per-issue search for every "missing" issue in the
/// catalog (no owned, present file) that has already shipped. Mirrors
/// the per-series Search-now's fire-and-forget contract — each issue
/// spawns its own `sweep_single_issue` task and reports its outcome to
/// the log. The engine's per-issue in-flight dedup keeps a re-click
/// from doubling the indexer load on already-running searches.
///
/// Watch out: this fans out to ALL non-solicited missing issues at
/// once. Per-indexer rate limits in the newznab clients absorb some of
/// the bombardment but a library with thousands of missing issues
/// will produce a sustained burst of indexer hits. Same fanout pattern
/// the per-series endpoint has had since A.8 — this just scales it to
/// the whole catalog.
async fn search_all_missing(
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    // Same "missing" predicate as routes/missing.rs and stats.rs — no
    // owned, present file matches this issue. We only need
    // (series_id, issue_id, cover_date); skip the join with the
    // series table because the per-issue search call doesn't use it.
    let rows = sqlx::query!(
        r#"SELECT i.id AS "issue_id!: i64",
                  i.series_id AS "series_id!: i64",
                  i.cover_date
           FROM issues i
           WHERE NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1
           )"#
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("missing query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;

    let today = time::OffsetDateTime::now_utc().date();
    let mut searched = 0usize;
    let mut skipped_solicited = 0usize;
    for row in rows {
        if is_solicited(row.cover_date.as_deref(), today) {
            skipped_solicited += 1;
            continue;
        }
        longbox_pull::fire_issue_search(state.db.clone(), row.series_id, row.issue_id);
        searched += 1;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(SearchAllMissingResponse {
            searched,
            skipped_solicited,
        }),
    )
        .into_response())
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

// -------- Pull-list export / import (Tier 4 ITEM 14) --------

/// One row in the exported pull-list payload. Carries enough to
/// re-link a subscription against a fresh catalog (cv_id is the
/// canonical identity), plus the human-readable title / year for
/// users browsing the export file directly.
#[derive(Debug, Serialize, sqlx::FromRow)]
struct PullListExportRow {
    series_id: i64,
    title: String,
    cv_id: Option<i64>,
    start_year: Option<i64>,
    subscribed_at: time::PrimitiveDateTime,
}

/// `GET /api/pull-list/export` — dump the user's current pull-list
/// subscriptions as a JSON array. Frontend triggers a file download.
/// The payload is the import endpoint's input contract — round-trip
/// safe between LongBox instances or backup/restore cycles.
async fn export_pull_list(
    State(state): State<AppState>,
) -> Result<Json<Vec<PullListExportRow>>, ApiError> {
    let rows = sqlx::query_as!(
        PullListExportRow,
        r#"SELECT
             p.series_id AS "series_id!: i64",
             s.title AS "title!: String",
             s.cv_id AS "cv_id?: i64",
             s.start_year AS "start_year?: i64",
             p.added_at AS "subscribed_at!: time::PrimitiveDateTime"
           FROM pull_list p
           JOIN series s ON s.id = p.series_id
           ORDER BY s.sort_title COLLATE NOCASE"#
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("pull-list export query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    Ok(Json(rows))
}

/// One row in an import payload. We accept the same shape `export`
/// produces; `series_id` and `subscribed_at` are accepted but ignored
/// — they're not portable between instances. The load-bearing field
/// is `cv_id`, which we use to look up the local series row.
#[derive(Debug, Deserialize)]
struct PullListImportRow {
    cv_id: Option<i64>,
    /// Display-only; surfaced in the per-entry result so a user can
    /// tell which line failed. Never written back to the catalog.
    title: Option<String>,
}

/// Per-entry outcome reported back to the caller. `added` is a
/// freshly-inserted subscription; `already_subscribed` short-circuits
/// the upsert; `series_not_found` means we have no local series for
/// this cv_id (user needs to add it first); `missing_cv_id` covers
/// shallow exports where the cv_id was null. Failure modes are not
/// fatal — every row is processed independently.
#[derive(Debug, Serialize)]
struct ImportEntryResult {
    cv_id: Option<i64>,
    title: Option<String>,
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ImportResponse {
    added: usize,
    already_subscribed: usize,
    series_not_found: usize,
    missing_cv_id: usize,
    results: Vec<ImportEntryResult>,
}

/// `POST /api/pull-list/import` — accept an array of rows in the
/// export format and add each to the pull list (upsert by cv_id).
/// Entries that reference an unknown cv_id (no matching series in the
/// catalog) are reported but not auto-created — the spec is "skip
/// duplicates," not "auto-add CV volumes." Per-entry results carry
/// the title back so the UI can render a useful summary.
async fn import_pull_list(
    State(state): State<AppState>,
    Json(entries): Json<Vec<PullListImportRow>>,
) -> Result<Json<ImportResponse>, ApiError> {
    use longbox_db::{pull_list_repo, series_repo, NewPullEntry};

    let mut added = 0usize;
    let mut already_subscribed = 0usize;
    let mut series_not_found = 0usize;
    let mut missing_cv_id = 0usize;
    let mut results = Vec::with_capacity(entries.len());

    for entry in entries {
        let Some(cv_id) = entry.cv_id else {
            missing_cv_id += 1;
            results.push(ImportEntryResult {
                cv_id: None,
                title: entry.title.clone(),
                status: "missing_cv_id",
            });
            continue;
        };
        let Some(series) = series_repo::find_by_cv_id(&state.db, cv_id).await? else {
            series_not_found += 1;
            results.push(ImportEntryResult {
                cv_id: Some(cv_id),
                title: entry.title.clone(),
                status: "series_not_found",
            });
            continue;
        };
        // Idempotency via the pull_list_repo's NotFound-vs-OK split:
        // `add` returns the new row on insert, and returns
        // `DbError::Conflict` (mapped from the UNIQUE constraint) when
        // already subscribed.
        match pull_list_repo::add(
            &state.db,
            NewPullEntry {
                series_id: series.id,
                start_issue: None,
            },
        )
        .await
        {
            Ok(_) => {
                added += 1;
                results.push(ImportEntryResult {
                    cv_id: Some(cv_id),
                    title: entry.title.clone(),
                    status: "added",
                });
            }
            Err(longbox_db::DbError::UniqueViolation {
                field: "pull_list_series_id",
            }) => {
                already_subscribed += 1;
                results.push(ImportEntryResult {
                    cv_id: Some(cv_id),
                    title: entry.title.clone(),
                    status: "already_subscribed",
                });
            }
            Err(e) => return Err(ApiError::from(e)),
        }
    }

    Ok(Json(ImportResponse {
        added,
        already_subscribed,
        series_not_found,
        missing_cv_id,
        results,
    }))
}
