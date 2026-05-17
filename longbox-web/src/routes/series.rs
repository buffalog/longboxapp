use axum::extract::{Path, State};
use axum::routing::{get, post};
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

    let volume = state.cv.fetch_volume(body.cv_id).await?;
    let cv_issues = state.cv.fetch_issues(body.cv_id).await?;

    // Project CV → domain inputs.
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

    // Fire-and-forget rematch. Does NOT touch scan_status. If the scanner
    // is currently busy, the spawned task gets ScanError::AlreadyRunning,
    // logs WARN, and exits. The series insert above already succeeded.
    let scanner = state.scanner.clone();
    let series_id = inserted.id;
    tokio::spawn(async move {
        match scanner.rematch_for_series(series_id).await {
            Ok(report) => tracing::info!(
                target: "longbox_web",
                series_id,
                matched = report.matched_owned,
                "add-series auto-rematch completed"
            ),
            Err(longbox_scanner::ScanError::AlreadyRunning) => tracing::warn!(
                target: "longbox_web",
                series_id,
                "add-series auto-rematch deferred: another scan is in progress"
            ),
            Err(e) => tracing::warn!(
                target: "longbox_web",
                series_id,
                error = %e,
                "add-series auto-rematch failed"
            ),
        }
    });

    Ok(Json(inserted))
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

    // Fire-and-forget rematch. Same silent-skip pattern as add-series:
    // refresh can introduce new issues (CV published #194 since last
    // refresh) and existing needs_review files may now match. If the
    // scanner is busy, AlreadyRunning logs WARN and exits. Does NOT
    // touch scan_status.
    let scanner = state.scanner.clone();
    let series_id = updated.id;
    tokio::spawn(async move {
        match scanner.rematch_for_series(series_id).await {
            Ok(report) => tracing::info!(
                target: "longbox_web",
                series_id,
                matched = report.matched_owned,
                "refresh-triggered auto-rematch completed"
            ),
            Err(longbox_scanner::ScanError::AlreadyRunning) => tracing::warn!(
                target: "longbox_web",
                series_id,
                "refresh-triggered auto-rematch deferred: another scan is in progress"
            ),
            Err(e) => tracing::warn!(
                target: "longbox_web",
                series_id,
                error = %e,
                "refresh-triggered auto-rematch failed"
            ),
        }
    });

    Ok(Json(updated))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if series_repo::find_by_id(&state.db, id).await?.is_none() {
        return Err(ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        });
    }
    // 409 if any owned files matched to this series's issues.
    let owned = sqlx::query!(
        r#"SELECT COUNT(*) AS n
           FROM files f
           JOIN issues i ON f.issue_id = i.id
           WHERE i.series_id = ? AND f.status = 'owned'"#,
        id
    )
    .fetch_one(&state.db)
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
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::Internal {
            message: format!("delete failed: {e}"),
            source: anyhow::anyhow!(e),
        })?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}
