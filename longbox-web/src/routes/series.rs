use axum::extract::{Path, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use longbox_core::{normalize_title, IssueNumber};
use longbox_db::{
    issue_repo, series_repo, IssueRow, NewIssue, NewSeries, SeriesRow, SeriesUpdate,
    SeriesWithCounts,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/series", post(add).get(list))
        .route("/series/:id", get(detail).delete(remove))
        .route("/series/:id/refresh", post(refresh))
        .route("/series/:id/cv-id", patch(set_cv_id))
}

// -------- shapes --------

#[derive(Debug, Deserialize)]
struct AddSeriesBody {
    cv_id: i64,
}

// SeriesListItem is now sourced directly from `longbox_db::SeriesWithCounts`,
// which carries the row + all five per-status counts. The handler just
// passes it through.

#[derive(Debug, Serialize)]
struct SeriesDetail {
    #[serde(flatten)]
    series: SeriesRow,
    issues: Vec<IssueWithFile>,
}

#[derive(Debug, Serialize)]
struct IssueWithFile {
    #[serde(flatten)]
    issue: IssueRow,
    file: Option<FileSummary>,
}

#[derive(Debug, Serialize)]
struct FileSummary {
    id: i64,
    path_relative: String,
    status: String,
    is_present: bool,
}

// -------- handlers --------

async fn add(
    State(state): State<AppState>,
    Json(body): Json<AddSeriesBody>,
) -> Result<Json<SeriesRow>, ApiError> {
    if body.cv_id <= 0 {
        return Err(ApiError::BadRequest {
            message: "cv_id must be > 0".into(),
        });
    }
    if let Some(existing) = series_repo::find_by_cv_id(&state.db, body.cv_id).await? {
        return Err(ApiError::Conflict {
            code: "conflict.series_already_exists",
            message: format!(
                "Series with cv_id {} already in watchlist (id={})",
                body.cv_id, existing.id
            ),
        });
    }

    let (inserted, _was_new) = add_or_get_from_cv(&state, body.cv_id).await?;
    spawn_auto_rematch(&state, inserted.id, "add-series");
    Ok(Json(inserted))
}

/// Shared series-creation helper used by `POST /api/series` and
/// `POST /api/files/:id/match-from-cv`. Returns the existing row if a
/// series with `cv_volume_id` is already in the watchlist (idempotent for
/// match-from-cv); the caller is responsible for any 409 / conflict policy
/// that's appropriate for its endpoint.
///
/// The `bool` is `true` when this call inserted the series and its issues
/// from a fresh ComicVine fetch; `false` when an existing series was
/// returned untouched (no CV calls were made).
pub(crate) async fn add_or_get_from_cv(
    state: &AppState,
    cv_volume_id: i64,
) -> Result<(SeriesRow, bool), ApiError> {
    if let Some(existing) = series_repo::find_by_cv_id(&state.db, cv_volume_id).await? {
        return Ok((existing, false));
    }

    let volume = state.cv.fetch_volume(cv_volume_id).await?;
    let cv_issues = state.cv.fetch_issues(cv_volume_id).await?;

    let new_series = NewSeries {
        cv_id: Some(volume.cv_id),
        metron_id: None,
        sort_title: normalize_title(&volume.name),
        title: volume.name,
        start_year: volume.start_year,
        publisher: volume.publisher,
        description: volume.description,
        cover_url: volume.cover_url,
    };

    let mut tx = state.db.begin().await.map_err(longbox_db::DbError::from)?;
    let inserted = series_repo::insert(&mut *tx, new_series).await?;
    let new_issues: Vec<NewIssue> = cv_issues
        .into_iter()
        .map(|i| NewIssue {
            series_id: inserted.id,
            cv_issue_id: Some(i.cv_issue_id),
            metron_issue_id: None,
            number: i.issue_number,
            title: i.name,
            cover_date: i.cover_date,
            summary: i.description,
            cover_url: i.cover_url,
        })
        .collect();
    if !new_issues.is_empty() {
        issue_repo::bulk_insert(&mut *tx, new_issues).await?;
    }
    tx.commit().await.map_err(longbox_db::DbError::from)?;

    Ok((inserted, true))
}

/// Fire-and-forget `rematch_for_series` with the standard log triage. Does
/// not touch scan_status. `caller` is a short tag (`"add-series"`,
/// `"refresh"`, `"match-from-cv"`) that gets baked into log messages.
pub(crate) fn spawn_auto_rematch(state: &AppState, series_id: i64, caller: &'static str) {
    let scanner = state.scanner.clone();
    tokio::spawn(async move {
        match scanner.rematch_for_series(series_id).await {
            Ok(report) => tracing::info!(
                target: "longbox_web",
                series_id,
                matched = report.matched_owned,
                caller,
                "auto-rematch completed"
            ),
            Err(longbox_scanner::ScanError::AlreadyRunning) => tracing::warn!(
                target: "longbox_web",
                series_id,
                caller,
                "auto-rematch deferred: another scan is in progress"
            ),
            Err(e) => tracing::warn!(
                target: "longbox_web",
                series_id,
                caller,
                error = %e,
                "auto-rematch failed"
            ),
        }
    });
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<SeriesWithCounts>>, ApiError> {
    Ok(Json(series_repo::find_all_with_counts(&state.db).await?))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<SeriesDetail>, ApiError> {
    let series = series_repo::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        })?;
    let mut issues = issue_repo::list_by_series(&state.db, id).await?;
    // Natural ordering by IssueNumber (e.g. "1" < "1.MU" < "Annual 1").
    issues.sort_by(|a, b| {
        let an = IssueNumber::new(a.number.clone());
        let bn = IssueNumber::new(b.number.clone());
        IssueNumber::natural_cmp(&an, &bn)
    });
    let mut with_files = Vec::with_capacity(issues.len());
    for issue in issues {
        let files = sqlx::query!(
            r#"SELECT id AS "id!: i64", path_relative, status, is_present AS "is_present!: bool"
               FROM files
               WHERE issue_id = ? AND is_present = 1
               ORDER BY id LIMIT 1"#,
            issue.id
        )
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::Internal {
            message: format!("issue file query failed: {e}"),
            source: anyhow::anyhow!(e),
        })?;
        let file = files.map(|f| FileSummary {
            id: f.id,
            path_relative: f.path_relative,
            status: f.status,
            is_present: f.is_present,
        });
        with_files.push(IssueWithFile { issue, file });
    }
    Ok(Json(SeriesDetail {
        series,
        issues: with_files,
    }))
}

#[derive(Debug, Deserialize)]
struct SetCvIdBody {
    cv_id: i64,
}

/// `PATCH /api/series/:id/cv-id` — Library Tidy disambiguation
/// surface. Sets the series's cv_id to the body-supplied value
/// (typically picked by the user from the CV search input on the
/// review-queue row), then re-fetches the volume + issues from CV
/// and overwrites the descriptive fields. Old issues are deleted
/// before the fresh insert: they belonged to the wrong volume and
/// would collide with the new issues' cv_issue_ids — and from the
/// catalog's perspective the series IS the new volume now, the
/// old issue numbers are meaningless.
///
/// Unlike `refresh`, this handler does NOT require an existing
/// cv_id on the series — that's the whole point of the
/// disambiguation flow (the queue is filtered to `cv_id IS NULL`
/// in the first place). It uses [`series_repo::force_set_cv_id`]
/// rather than `set_cv_id` so the call works whether or not the
/// row already has one.
async fn set_cv_id(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<SetCvIdBody>,
) -> Result<Json<SeriesRow>, ApiError> {
    if body.cv_id <= 0 {
        return Err(ApiError::BadRequest {
            message: "cv_id must be > 0".into(),
        });
    }
    let _existing = series_repo::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        })?;
    // Pre-check the UNIQUE(cv_id) collision so we can return a
    // structured 409 instead of letting the DB constraint surface
    // as a generic Internal. Race-window between this and the
    // UPDATE is closed by the DB-level UNIQUE backstop — that path
    // surfaces as Internal but the common-case error is clean.
    if let Some(other) = series_repo::find_by_cv_id(&state.db, body.cv_id).await? {
        if other.id != id {
            return Err(ApiError::Conflict {
                code: "conflict.cv_id_in_use",
                message: format!(
                    "cv_id {} is already linked to series {} ({:?})",
                    body.cv_id, other.id, other.title
                ),
            });
        }
    }

    // Fetch BEFORE any DB write. If CV is down or rate-limited the
    // user can retry without the catalog landing in a half-wiped
    // state.
    // Use the unthrottled cv_direct client — this is a user-initiated
    // interactive request; it must not queue behind the enrichment worker's
    // 30-60s rate-limiter slot wait.
    let volume = state.cv_direct.fetch_volume(body.cv_id).await?;
    let cv_issues = state.cv_direct.fetch_issues(body.cv_id).await?;

    let mut tx = state.db.begin().await.map_err(longbox_db::DbError::from)?;

    let rows_updated = series_repo::force_set_cv_id(&mut *tx, id, body.cv_id).await?;
    if rows_updated == 0 {
        // The series row was deleted between the existence check
        // and the UPDATE — rare, but possible if a tidy action
        // ran concurrently. Surface as 404 (the disambiguation
        // target is gone).
        return Err(ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        });
    }

    let update_input = SeriesUpdate {
        title: volume.name.clone(),
        sort_title: normalize_title(&volume.name),
        start_year: volume.start_year,
        publisher: volume.publisher,
        description: volume.description,
        cover_url: volume.cover_url,
    };
    let updated = series_repo::update(&mut *tx, id, update_input).await?;

    issue_repo::delete_by_series(&mut *tx, id).await?;
    let new_issues: Vec<NewIssue> = cv_issues
        .into_iter()
        .map(|i| NewIssue {
            series_id: id,
            cv_issue_id: Some(i.cv_issue_id),
            metron_issue_id: None,
            number: i.issue_number,
            title: i.name,
            cover_date: i.cover_date,
            summary: i.description,
            cover_url: i.cover_url,
        })
        .collect();
    if !new_issues.is_empty() {
        issue_repo::bulk_insert(&mut *tx, new_issues).await?;
    }

    tx.commit().await.map_err(longbox_db::DbError::from)?;

    // The new issues need rematching against the catalog files
    // that were owned-but-pointing-at-the-now-deleted issues.
    spawn_auto_rematch(&state, id, "set-cv-id");
    Ok(Json(updated))
}

async fn refresh(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<SeriesRow>, ApiError> {
    let existing = series_repo::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        })?;
    let cv_id = existing.cv_id.ok_or_else(|| ApiError::BadRequest {
        message: "series has no cv_id; cannot refresh from ComicVine".into(),
    })?;

    let volume = state.cv.fetch_volume(cv_id).await?;
    let cv_issues = state.cv.fetch_issues(cv_id).await?;

    // Overwrite mutable fields via series_repo::update (identity columns —
    // cv_id, metron_id — are not in SeriesUpdate).
    let update_input = SeriesUpdate {
        title: volume.name.clone(),
        sort_title: longbox_core::normalize_title(&volume.name),
        start_year: volume.start_year,
        publisher: volume.publisher,
        description: volume.description,
        cover_url: volume.cover_url,
    };
    let mut tx = state.db.begin().await.map_err(longbox_db::DbError::from)?;
    let updated = series_repo::update(&mut *tx, id, update_input).await?;

    // Upsert issues by cv_issue_id so existing issues get refreshed and new
    // ones are added.
    for cv_issue in cv_issues {
        issue_repo::upsert_by_cv_id(
            &mut *tx,
            NewIssue {
                series_id: updated.id,
                cv_issue_id: Some(cv_issue.cv_issue_id),
                metron_issue_id: None,
                number: cv_issue.issue_number,
                title: cv_issue.name,
                cover_date: cv_issue.cover_date,
                summary: cv_issue.description,
                cover_url: cv_issue.cover_url,
            },
        )
        .await?;
    }
    tx.commit().await.map_err(longbox_db::DbError::from)?;

    // Refresh can introduce new issues (CV published #194 since last
    // refresh) and existing needs_review files may now match.
    spawn_auto_rematch(&state, updated.id, "refresh");
    Ok(Json(updated))
}

/// Delete a single series, enforcing the owned-files guard. Shared by
/// `DELETE /api/series/:id` and the Library Tidy phantom-delete routes
/// (`routes/reconcile.rs`) so both enforce identical semantics from one
/// place.
///
/// Returns `NotFound` for an unknown id and `Conflict` when owned files
/// are *present on disk* for the series. For a phantom delete the
/// conflict is a real time-of-check/time-of-use guard — a series can
/// regain files between the tidy view loading and the delete being
/// clicked. On success the `series` row is deleted; dependent
/// `issues`/`files` rows cascade.
///
/// The guard counts `status = 'owned' AND is_present = 1`. The
/// `is_present = 1` clause is load-bearing: a *transition phantom* has
/// owned files whose `is_present` the scanner's mark-missing pass
/// flipped to 0 (it never touches `status`), so without the clause this
/// guard would 409 every transition phantom — the exact rows Library
/// Tidy exists to let the user delete. Counting only present owned
/// files is also the correct `DELETE /api/series/:id` semantic: a series
/// whose files merely went missing should be deletable.
pub(crate) async fn delete_series(db: &longbox_db::Pool, id: i64) -> Result<(), ApiError> {
    if series_repo::find_by_id(db, id).await?.is_none() {
        return Err(ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        });
    }
    // 409 if any owned files are present on disk for this series's issues.
    let owned = sqlx::query!(
        r#"SELECT COUNT(*) AS n
           FROM files f
           JOIN issues i ON f.issue_id = i.id
           WHERE i.series_id = ? AND f.status = 'owned' AND f.is_present = 1"#,
        id
    )
    .fetch_one(db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("owned-file count failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    if owned.n > 0 {
        return Err(ApiError::Conflict {
            code: "conflict.series_has_owned_files",
            message: format!(
                "{} owned file(s) match issues in this series. Remove or ignore them before deleting.",
                owned.n
            ),
        });
    }
    sqlx::query!(r#"DELETE FROM series WHERE id = ?"#, id)
        .execute(db)
        .await
        .map_err(|e| ApiError::Internal {
            message: format!("delete failed: {e}"),
            source: anyhow::anyhow!(e),
        })?;
    Ok(())
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    delete_series(&state.db, id).await?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}
