use std::path::{Path as StdPath, PathBuf};

use axum::extract::{Path, Query, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use longbox_core::{normalize_title, IssueNumber};
use longbox_db::{
    downloader_config_repo, indexer_config_repo, issue_repo, library_root_repo, series_repo,
    settings_repo, IssueRow, NewIssue, NewSeries, SeriesRow, SeriesUpdate, SeriesWithCounts,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/series", post(add).get(list))
        .route("/series/:id", get(detail).delete(remove))
        .route("/series/:id/refresh", post(refresh))
        .route("/series/:id/cv-id", patch(set_cv_id))
        .route("/series/:id/folder-path", get(folder_path))
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
    /// Owned-and-present file count for this series, sourced from
    /// the same SQL the `delete_series` guard uses (`files JOIN
    /// issues WHERE series_id=? AND status='owned' AND
    /// is_present=1`). Sidestepped the bug where the frontend
    /// derived the count from `issues[].file` and underreported on
    /// shallow series — the modal-skip guard would fire on a
    /// false 0 and the delete would 409 because the backend
    /// counted real owned files via the join. With this field the
    /// frontend reads the same number the guard does.
    owned_file_count: i64,
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

/// Response body for `POST /api/series`. Flattens the inserted
/// `SeriesRow` so existing frontend reads of fields like `id` /
/// `title` keep working; the new `pull_search_queued` field carries
/// the count of issues for which a search task was just fired so the
/// frontend can render a "searching for N missing issues" toast. The
/// field is `0` when the pull engine isn't configured (no downloader
/// or no enabled indexers) — the auto-search silently skips per the
/// product spec.
#[derive(Debug, Serialize)]
struct AddSeriesResponse {
    #[serde(flatten)]
    series: SeriesRow,
    pull_search_queued: usize,
}

async fn add(
    State(state): State<AppState>,
    Json(body): Json<AddSeriesBody>,
) -> Result<Json<AddSeriesResponse>, ApiError> {
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
            details: serde_json::Value::Null,
        });
    }

    let (inserted, _was_new) = add_or_get_from_cv(&state, body.cv_id).await?;

    // Pre-create the on-disk series folder so the user can drop files
    // into it immediately without waiting for the next scan or for the
    // first file to land via Phase B. Idempotent — `create_dir_all` is
    // a no-op on an existing directory and handles any intermediate
    // directories along the way. Best-effort: a failure here does NOT
    // unwind the DB insert (the series is in the catalog regardless),
    // it just lands as a WARN log; the operator can mkdir manually.
    let library_root = StdPath::new(&state.config.library_root_path);
    let folder = series_folder_path(library_root, &inserted);
    match std::fs::create_dir_all(&folder) {
        Ok(()) => tracing::info!(
            target: "longbox_web",
            series_id = inserted.id,
            series_title = %inserted.title,
            folder = %folder.display(),
            "add-series.folder_ready"
        ),
        Err(e) => tracing::warn!(
            target: "longbox_web",
            series_id = inserted.id,
            series_title = %inserted.title,
            folder = %folder.display(),
            err = %e,
            "add-series.folder_create_failed"
        ),
    }

    spawn_auto_rematch(&state, inserted.id, "add-series");

    // Auto-search: same semantics as the Search-missing button on
    // series detail, fired automatically as part of the add flow.
    // Silent-skip when the pull engine can't actually do anything (no
    // downloader configured, downloader disabled, or no enabled
    // indexers) so a freshly-added series during a paused-pull-engine
    // period doesn't claim "searching N issues" when it actually
    // isn't.
    let pull_search_queued =
        spawn_auto_pull_search(&state, inserted.id, &inserted.title).await?;

    Ok(Json(AddSeriesResponse {
        series: inserted,
        pull_search_queued,
    }))
}

/// Pre-flight the pull engine's configuration AND dispatch a search
/// task for every missing-and-shipped issue of the just-added series.
/// Returns the number of issues for which `fire_issue_search` was
/// called. Returns 0 — without any disk or indexer side-effects —
/// when the pull engine isn't fully configured.
async fn spawn_auto_pull_search(
    state: &AppState,
    series_id: i64,
    series_title: &str,
) -> Result<usize, ApiError> {
    let downloader = match downloader_config_repo::get(&state.db).await? {
        Some(d) if d.enabled => d,
        Some(_) => {
            tracing::info!(
                target: "longbox_web",
                series_id,
                "add-series.auto_search_skipped (downloader disabled)"
            );
            return Ok(0);
        }
        None => {
            tracing::info!(
                target: "longbox_web",
                series_id,
                "add-series.auto_search_skipped (no downloader configured)"
            );
            return Ok(0);
        }
    };
    let _ = downloader;
    let indexers = indexer_config_repo::list_enabled(&state.db).await?;
    if indexers.is_empty() {
        tracing::info!(
            target: "longbox_web",
            series_id,
            "add-series.auto_search_skipped (no enabled indexers)"
        );
        return Ok(0);
    }
    // Same predicate the per-series Search-missing button uses
    // (`issue_repo::list_missing_for_series`): cover_date present and
    // `<= today` (shipped), no owned-and-present file. The freshly-
    // added series typically has every issue in this state.
    let missing = issue_repo::list_missing_for_series(&state.db, series_id).await?;
    if missing.is_empty() {
        return Ok(0);
    }
    let queued = missing.len();
    for issue in missing {
        longbox_pull::fire_issue_search(state.db.clone(), series_id, issue.id);
    }
    tracing::info!(
        target: "longbox_web",
        series_id,
        series_title,
        queued,
        "add-series.auto_search_dispatched"
    );
    Ok(queued)
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

/// Fire-and-forget `rescan_unmatched` for the configured library root.
/// Used by `DELETE /api/series/:id?force=true` after unlinking the
/// duplicate's files — the freshly `needs_review`-flagged files need
/// to be matched against the rest of the catalog, NOT against a
/// specific series (we don't know which real series they belong to;
/// the catalog cascade will figure it out).
pub(crate) fn spawn_rescan_unmatched(state: &AppState, caller: &'static str) {
    let scanner = state.scanner.clone();
    let library_root_id = state.library_root_id;
    tokio::spawn(async move {
        match scanner.rescan_unmatched(library_root_id).await {
            Ok(report) => tracing::info!(
                target: "longbox_web",
                library_root_id,
                matched_owned = report.matched_owned,
                matched_needs_review = report.matched_needs_review,
                caller,
                "rescan-unmatched completed"
            ),
            Err(longbox_scanner::ScanError::AlreadyRunning) => tracing::warn!(
                target: "longbox_web",
                library_root_id,
                caller,
                "rescan-unmatched deferred: another scan is in progress"
            ),
            Err(e) => tracing::warn!(
                target: "longbox_web",
                library_root_id,
                caller,
                error = %e,
                "rescan-unmatched failed"
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
    // Authoritative owned-files count. Same predicate as the
    // delete_series guard (issue→series join, status='owned',
    // is_present=1) so the frontend's modal-skip decision aligns
    // with what the backend will actually do on the delete call.
    let owned_file_count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64"
           FROM files f
           JOIN issues i ON f.issue_id = i.id
           WHERE i.series_id = ?
             AND f.status = 'owned'
             AND f.is_present = 1"#,
        id
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("owned-file count query failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;

    Ok(Json(SeriesDetail {
        series,
        issues: with_files,
        owned_file_count,
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
///
/// # Transactional contract
///
/// 1. ComicVine fetches (`fetch_volume`, `fetch_issues`) happen
///    BEFORE `tx.begin()`. If CV is down, rate-limited, or returns
///    an error mid-pagination, `?` short-circuits and no DB row has
///    been touched. The user retries.
/// 2. Every catalog mutation (cv_id update, series row update, old
///    issue delete, new issue bulk insert, file unlink-for-rematch)
///    runs inside ONE `tx`. Any `await?` inside the body drops the
///    tx without commit → automatic rollback. The series never lands
///    in a half-wiped state.
/// 3. `spawn_auto_rematch` runs POST-commit (fire-and-forget). It
///    only sees the committed catalog state. The file-unlink step
///    inside the tx is load-bearing — without it, the cascade
///    `ON DELETE SET NULL` on `files.issue_id` would leave every
///    formerly-linked file as `issue_id = NULL, status = 'owned'`,
///    a stuck-orphan state that `rematch_for_series` would skip
///    because it only scans `needs_review`. Flipping files to
///    `needs_review` atomically with the issue wipe is what lets
///    the post-commit rematch actually link them.
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
    let existing = series_repo::find_by_id(&state.db, id)
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
                    "This ComicVine volume is already linked to \"{}\" (series #{}).",
                    other.title, other.id
                ),
                details: serde_json::json!({
                    "existing_series_id": other.id,
                    "existing_series_title": other.title,
                }),
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

    // Reset every file under the pre-PATCH series folder so the
    // rematch step (below) actually finds them. Without this step
    // the rematch is a no-op for this surface: `issues` was deleted,
    // and the `files.issue_id ON DELETE SET NULL` cascade left every
    // formerly-linked file at `issue_id = NULL, status = 'owned'` —
    // an orphan state where `rematch_for_series` skips them because
    // it only scans `needs_review`. Path-based filter using the PRE-
    // PATCH title + start_year because the folder on disk is named
    // whatever the catalog had when the files were attached; the
    // PATCH just updated the catalog title to the new CV volume's
    // name, but the on-disk folder is unchanged.
    //
    // `substr(path_relative, 1, N) = prefix` is byte-exact prefix
    // matching (no LIKE wildcards, no escape gymnastics). Length is
    // SQLite's `length(prefix)` — which counts characters for TEXT,
    // matching `prefix.chars().count()` in Rust.
    let folder_prefix = match existing.start_year {
        Some(year) => format!("{} ({})/", existing.title, year),
        None => format!("{}/", existing.title),
    };
    let prefix_chars = i64::try_from(folder_prefix.chars().count()).unwrap_or(i64::MAX);
    let library_root_id = state.library_root_id;
    sqlx::query!(
        r#"UPDATE files
             SET issue_id = NULL,
                 status = 'needs_review'
           WHERE library_root_id = ?
             AND substr(path_relative, 1, ?) = ?"#,
        library_root_id,
        prefix_chars,
        folder_prefix
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("file rematch reset failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;

    tx.commit().await.map_err(longbox_db::DbError::from)?;

    // The new issues need rematching against the catalog files
    // that were owned-but-pointing-at-the-now-deleted issues. The
    // pre-commit UPDATE above flipped those files to `needs_review`
    // so `rematch_for_series` actually sees them.
    spawn_auto_rematch(&state, id, "set-cv-id");
    Ok(Json(updated))
}

#[derive(Debug, Serialize)]
struct FolderPathResponse {
    /// Path as the container sees it (e.g. `/library/Saga (2012)`).
    /// Always present — derived from `series.title` + `start_year`
    /// joined under the library root, same convention every other
    /// folder-aware code path uses (`series_folder_path`,
    /// delete_files, snapshot helpers).
    container_path: String,
    /// Host-side path with the `host_library_path` setting
    /// substituted for the container's library_root prefix. Equal to
    /// `container_path` when the setting is unset. The frontend uses
    /// this to render a `file://` URL the host OS can resolve to a
    /// Finder/Explorer window.
    host_path: String,
    /// Convenience flag for the frontend: true iff `host_library_path`
    /// is configured, so the UI can render an active "Show in Finder"
    /// link vs a copy-only display.
    host_path_configured: bool,
}

/// `GET /api/series/:id/folder-path` — compute the disk location of
/// the series folder, returning both the container path and the
/// host-OS path (when a `host_library_path` setting is configured).
/// The frontend's "Show in Finder" affordance uses `host_path` to
/// open a `file://` URL; the container path is along for the ride so
/// users without a configured host path still get something
/// copy-pastable into Cmd+Shift+G.
async fn folder_path(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<FolderPathResponse>, ApiError> {
    let series = series_repo::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        })?;
    let root_row = library_root_repo::find_by_id(&state.db, state.library_root_id)
        .await?
        .ok_or_else(|| ApiError::Internal {
            message: format!(
                "library_root_id {} on AppState resolves to no row",
                state.library_root_id
            ),
            source: anyhow::anyhow!("library root row missing"),
        })?;

    // Container-side path: the same `{title} ({year})/` (or bare
    // `{title}/`) convention `series_folder_path` uses for delete +
    // backup. Strip any trailing slash off the library root so the
    // join doesn't double-slash.
    let library_root_trimmed = root_row.path.trim_end_matches('/');
    let folder_name = match series.start_year {
        Some(year) => format!("{} ({})", series.title, year),
        None => series.title.clone(),
    };
    let container_path = format!("{library_root_trimmed}/{folder_name}");

    // Host-side path: substitute the container's library root prefix
    // with the configured host path. Both sides get trimmed of
    // trailing slashes so the substitution doesn't produce
    // `/host//Saga (2012)`.
    let host_library_path: String = settings_repo::get_or_default(
        &state.db,
        settings_repo::KEY_HOST_LIBRARY_PATH,
        String::new(),
    )
    .await?;
    let host_library_path_trimmed = host_library_path.trim().trim_end_matches('/');
    let host_path_configured = !host_library_path_trimmed.is_empty();
    let host_path = if host_path_configured {
        format!("{host_library_path_trimmed}/{folder_name}")
    } else {
        container_path.clone()
    };

    Ok(Json(FolderPathResponse {
        container_path,
        host_path,
        host_path_configured,
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
            details: serde_json::Value::Null,
        });
    }
    delete_series_row(db, id).await
}

/// Bare `DELETE FROM series` with no owned-files guard and no existence
/// check. The handler's `delete_files=true` path uses this directly — the
/// guard exists to stop the user from accidentally orphaning bytes on
/// disk, but `delete_files=true` is the explicit "yes, delete the bytes
/// too" opt-in, so applying the guard there is the order-of-operations
/// bug that surfaced as a 409 when the frontend's modal sent the flag
/// after confirmation. Callers MUST verify existence first when 404 is
/// the desired outcome for a missing id.
pub(crate) async fn delete_series_row(
    db: &longbox_db::Pool,
    id: i64,
) -> Result<(), ApiError> {
    sqlx::query!(r#"DELETE FROM series WHERE id = ?"#, id)
        .execute(db)
        .await
        .map_err(|e| ApiError::Internal {
            message: format!("delete failed: {e}"),
            source: anyhow::anyhow!(e),
        })?;
    Ok(())
}

/// Force-delete variant used by the Library Tidy "Delete duplicate
/// anyway" affordance. The caller has accepted that the owned files
/// presently linked to this series are misassigned (the real series is
/// elsewhere in the catalog) and wants them returned to the unmatched
/// pool. Unlinks every file from the series's issues — `issue_id` to
/// NULL and `status` to `needs_review` — then deletes the series; the
/// `issues` cascade does the rest. The owned-files guard is
/// deliberately bypassed.
///
/// The caller is responsible for kicking off a rescan after this
/// returns so the now-`needs_review` files find their real home.
pub(crate) async fn force_delete_series(
    db: &longbox_db::Pool,
    id: i64,
) -> Result<(), ApiError> {
    if series_repo::find_by_id(db, id).await?.is_none() {
        return Err(ApiError::NotFound {
            resource: "series",
            id: id.to_string(),
        });
    }
    let mut tx = db.begin().await.map_err(longbox_db::DbError::from)?;
    sqlx::query!(
        r#"UPDATE files
             SET issue_id = NULL,
                 status = 'needs_review'
           WHERE issue_id IN (SELECT id FROM issues WHERE series_id = ?)"#,
        id
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal {
        message: format!("file-unlink failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    sqlx::query!(r#"DELETE FROM series WHERE id = ?"#, id)
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::Internal {
            message: format!("delete failed: {e}"),
            source: anyhow::anyhow!(e),
        })?;
    tx.commit().await.map_err(longbox_db::DbError::from)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    force: bool,
    /// When `true`, after the DB delete succeeds the handler also
    /// removes the series's on-disk folder via `std::fs::remove_dir_all`.
    /// Off by default: the prior delete contract was DB-only, and an
    /// API consumer or script must opt in to destructive disk behavior.
    /// Incompatible with `force`: the force path exists for misassigned
    /// files (the real series lives elsewhere), so wiping the
    /// title-based folder would destroy bytes that belong to ANOTHER
    /// series. The handler rejects the combination with 400.
    #[serde(default)]
    delete_files: bool,
}

/// Compute the on-disk series folder under a library root from
/// `series.title` and `series.start_year`:
///   `{library_root}/{title} ({year})/`  when year is present
///   `{library_root}/{title}/`            when year is null
/// Matches the convention used by every other code path that names
/// series folders (e.g. Mylar imports, manual organization).
///
/// Returned path is the JOIN result; safety verification — that it
/// actually resolves under `library_root` after canonicalization, isn't
/// a symlink, etc. — is the caller's job via
/// [`canonicalize_within_root`].
fn series_folder_path(library_root: &StdPath, series: &SeriesRow) -> PathBuf {
    let name = match series.start_year {
        Some(year) => format!("{} ({})", series.title, year),
        None => series.title.clone(),
    };
    library_root.join(name)
}

/// Resolve `candidate` and verify it is a real, non-symlink directory
/// strictly under `library_root` (also resolved). The canonicalization
/// step is what defeats path traversal — a series title like `../etc`
/// builds a `candidate` that lexically escapes the root, but
/// `canonicalize` returns the actual filesystem path and we reject any
/// result that doesn't sit under the canonicalized root.
///
/// Returns:
///   `Ok(Some(path))` — verified, safe to `remove_dir_all`.
///   `Ok(None)`       — folder doesn't exist on disk (best-effort: log
///                      and skip; the DB delete already succeeded).
///   `Err(...)`       — path is a symlink, a non-directory, or escapes
///                      the library root after canonicalization.
fn canonicalize_within_root(
    library_root: &StdPath,
    candidate: &StdPath,
) -> Result<Option<PathBuf>, ApiError> {
    // The DB delete has already happened by the time this runs, so a
    // missing folder is a soft outcome (best-effort): warn-and-skip.
    let metadata = match std::fs::symlink_metadata(candidate) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ApiError::Internal {
                message: format!("stat series folder failed: {e}"),
                source: anyhow::anyhow!(e),
            });
        }
    };
    // Symlink → refuse. Following it could land anywhere on disk —
    // outside the library root, in /etc, in the user's home. Better to
    // fail loudly and let the user remove the symlink manually.
    if metadata.file_type().is_symlink() {
        return Err(ApiError::Conflict {
            code: "conflict.series_folder_is_symlink",
            message: format!(
                "Refusing to delete {} — the folder is a symlink. Remove it manually.",
                candidate.display()
            ),
            details: serde_json::Value::Null,
        });
    }
    if !metadata.is_dir() {
        return Err(ApiError::Conflict {
            code: "conflict.series_folder_not_a_directory",
            message: format!(
                "Refusing to delete {} — the path is not a directory.",
                candidate.display()
            ),
            details: serde_json::Value::Null,
        });
    }
    let canonical_root = std::fs::canonicalize(library_root).map_err(|e| ApiError::Internal {
        message: format!("canonicalize library_root failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    let canonical_target = std::fs::canonicalize(candidate).map_err(|e| ApiError::Internal {
        message: format!("canonicalize series folder failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(ApiError::Conflict {
            code: "conflict.series_folder_outside_library_root",
            message: format!(
                "Refusing to delete {} — it resolves outside the library root after canonicalization.",
                candidate.display()
            ),
            details: serde_json::Value::Null,
        });
    }
    // Belt-and-braces: do not let the canonical target *be* the library
    // root itself. A pathological series with empty title joined with
    // `library_root` would otherwise nuke the entire library.
    if canonical_target == canonical_root {
        return Err(ApiError::Conflict {
            code: "conflict.series_folder_is_library_root",
            message: "Refusing to delete the library root itself.".into(),
            details: serde_json::Value::Null,
        });
    }
    Ok(Some(canonical_target))
}

/// Derive a pre-delete backup target from `DATABASE_URL`. Supports the
/// three sqlx URL shapes we use:
///   `sqlite::memory:`    → `None` (nothing to back up)
///   `sqlite:foo.db`      → `foo.db.pre-delete-backup`
///   `sqlite:/data/db`    → `/data/db.pre-delete-backup`
///   `sqlite:./db?mode=…` → `./db.pre-delete-backup`
/// Query-string suffix (`?mode=rwc`) is stripped before computing the
/// filesystem path. Returns `None` for any URL we can't confidently
/// resolve to a single on-disk file — the caller skips the backup
/// rather than guessing.
fn derive_db_backup_path(database_url: &str) -> Option<PathBuf> {
    let no_scheme = database_url.strip_prefix("sqlite:")?;
    let path_part = no_scheme.split('?').next()?;
    // Tolerate `sqlite://path` (URL-style) in addition to the bare
    // `sqlite:path` we usually use.
    let path_part = path_part.trim_start_matches("//");
    if path_part.is_empty() || path_part == ":memory:" {
        return None;
    }
    let mut backup = PathBuf::from(path_part);
    let name = backup.file_name()?.to_owned();
    let mut backup_name = name.into_string().ok()?;
    backup_name.push_str(".pre-delete-backup");
    backup.set_file_name(backup_name);
    Some(backup)
}

/// Capture an atomic, snapshot-consistent copy of the SQLite DB to the
/// derived backup path. Uses `VACUUM INTO` rather than `std::fs::copy`
/// because the latter can pick up mid-write state when writers are
/// active — VACUUM INTO produces a clean file even under concurrent
/// load. The target file is removed first if it exists (VACUUM INTO
/// refuses to overwrite). Spec says one backup file, overwrite each
/// destructive op.
///
/// Backup failure is fatal for the destructive op: if we can't write
/// the safety net, we don't take the destructive action. The user
/// gets a clear error and can re-try after fixing disk/permissions.
///
/// Returns the path written to so the caller can log it; `Ok(None)`
/// when the DB URL has no on-disk path (in-memory tests, mostly).
async fn snapshot_db_for_destructive_op(
    state: &AppState,
) -> Result<Option<PathBuf>, ApiError> {
    let Some(backup_path) = derive_db_backup_path(&state.config.database_url) else {
        return Ok(None);
    };
    // VACUUM INTO will not overwrite an existing target; remove it first
    // so the spec's "one backup file, overwrite each delete" holds.
    if backup_path.exists() {
        std::fs::remove_file(&backup_path).map_err(|e| ApiError::Internal {
            message: format!(
                "could not remove stale DB backup at {}: {e}",
                backup_path.display()
            ),
            source: anyhow::anyhow!(e),
        })?;
    }
    // VACUUM INTO's target path is a SQL string literal — no bind
    // parameter support — so escape any single quotes by doubling per
    // SQLite rules. Without this a path containing `'` would break the
    // statement.
    let path_str = backup_path
        .to_str()
        .ok_or_else(|| ApiError::Internal {
            message: format!(
                "backup path is not valid UTF-8: {}",
                backup_path.display()
            ),
            source: anyhow::anyhow!("non-UTF-8 backup path"),
        })?
        .replace('\'', "''");
    let stmt = format!("VACUUM INTO '{}'", path_str);
    sqlx::query(&stmt)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::Internal {
            message: format!("VACUUM INTO {} failed: {e}", backup_path.display()),
            source: anyhow::anyhow!(e),
        })?;
    Ok(Some(backup_path))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<RemoveQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if q.force && q.delete_files {
        return Err(ApiError::BadRequest {
            message: "force=true and delete_files=true are mutually exclusive: the force path \
                      exists for misassigned files whose real series lives elsewhere, so deleting \
                      the title-based folder would destroy bytes belonging to another series."
                .into(),
        });
    }

    // For delete_files we need to read the series row (for title +
    // start_year) BEFORE the DB delete cascades it away.
    let to_remove = if q.delete_files {
        Some(series_repo::find_by_id(&state.db, id).await?.ok_or(
            ApiError::NotFound {
                resource: "series",
                id: id.to_string(),
            },
        )?)
    } else {
        None
    };

    // Pre-delete safety net: when we're about to wipe bytes from disk
    // (delete_files=true), capture an atomic snapshot of the catalog
    // first. The user can recover from `{DATABASE_URL}.pre-delete-backup`
    // if they discover they deleted the wrong series. Backup failure is
    // fatal — without a safety net we don't take the destructive
    // action. Plain delete (no files touched) doesn't need this; the
    // catalog mutation is recoverable from the normal scan/rematch
    // pipeline if the user re-creates the series.
    if let Some(ref series) = to_remove {
        if let Some(path) = snapshot_db_for_destructive_op(&state).await? {
            warn!(
                target: "longbox_web",
                series_id = id,
                series_title = %series.title,
                backup = %path.display(),
                "pre-delete DB snapshot written"
            );
        }
    }

    if q.force {
        force_delete_series(&state.db, id).await?;
        spawn_rescan_unmatched(&state, "force-delete-series");
    } else if q.delete_files {
        // delete_files=true is the explicit user opt-in to disk-side
        // cleanup; the owned-files guard exists to stop accidental
        // bytes-orphaning, which doesn't apply here — we're about to
        // remove those bytes ourselves. Existence is already verified
        // upstream via the find_by_id that populated `to_remove`, so
        // 404-mapping isn't needed here either.
        delete_series_row(&state.db, id).await?;
    } else {
        delete_series(&state.db, id).await?;
    }

    let mut folder_deleted: Option<String> = None;
    if let Some(series) = to_remove {
        let root_row = library_root_repo::find_by_id(&state.db, state.library_root_id)
            .await?
            .ok_or(ApiError::Internal {
                message: format!(
                    "library_root_id {} on AppState resolves to no row",
                    state.library_root_id
                ),
                source: anyhow::anyhow!("library root row missing"),
            })?;
        let library_root_path = PathBuf::from(&root_row.path);
        let candidate = series_folder_path(&library_root_path, &series);
        match canonicalize_within_root(&library_root_path, &candidate)? {
            Some(safe_path) => {
                // Logged at WARN so it shows up in default-verbosity
                // container logs alongside other destructive actions
                // (admin restart, force-delete) — operators see
                // disk-touching ops without scraping debug logs.
                warn!(
                    target: "longbox_web",
                    series_id = id,
                    series_title = %series.title,
                    folder = %safe_path.display(),
                    "removing series folder from disk"
                );
                std::fs::remove_dir_all(&safe_path).map_err(|e| ApiError::Internal {
                    message: format!(
                        "remove_dir_all({}) failed: {e}",
                        safe_path.display()
                    ),
                    source: anyhow::anyhow!(e),
                })?;
                folder_deleted = Some(safe_path.display().to_string());
            }
            None => {
                // Folder absent: the convention didn't match (manual
                // rename, CV-canonical name, never on disk to begin
                // with). Warn so the operator can investigate; the
                // delete still counts as successful — DB is consistent.
                warn!(
                    target: "longbox_web",
                    series_id = id,
                    series_title = %series.title,
                    folder = %candidate.display(),
                    "series folder not found on disk; DB delete succeeded but no bytes were removed"
                );
            }
        }
    }

    let mut body = serde_json::json!({ "deleted": id });
    if let Some(path) = folder_deleted {
        body["folder_deleted"] = serde_json::Value::String(path);
    }
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_path_skipped_for_memory_db() {
        assert_eq!(derive_db_backup_path("sqlite::memory:"), None);
    }

    #[test]
    fn backup_path_skipped_for_unknown_scheme() {
        assert_eq!(derive_db_backup_path("postgres://foo"), None);
        assert_eq!(derive_db_backup_path(""), None);
    }

    #[test]
    fn backup_path_for_bare_filename() {
        assert_eq!(
            derive_db_backup_path("sqlite:longbox.db"),
            Some(PathBuf::from("longbox.db.pre-delete-backup"))
        );
    }

    #[test]
    fn backup_path_for_absolute_path() {
        assert_eq!(
            derive_db_backup_path("sqlite:/data/longbox.db"),
            Some(PathBuf::from("/data/longbox.db.pre-delete-backup"))
        );
    }

    #[test]
    fn backup_path_strips_query_string() {
        // The mode=rwc suffix is sqlx connection config — must not end
        // up in the backup filename.
        assert_eq!(
            derive_db_backup_path("sqlite:./longbox.db?mode=rwc"),
            Some(PathBuf::from("./longbox.db.pre-delete-backup"))
        );
    }

    #[test]
    fn backup_path_accepts_url_style_double_slash() {
        // `sqlite://path` is the URL-style variant some sqlx versions
        // accept; treat it the same as the bare `sqlite:path` form.
        assert_eq!(
            derive_db_backup_path("sqlite:///abs/path.db"),
            Some(PathBuf::from("/abs/path.db.pre-delete-backup"))
        );
    }

    /// End-to-end check on a real file-backed SQLite DB: VACUUM INTO
    /// writes a usable snapshot, and a second call cleanly overwrites
    /// the previous backup (the spec's "one backup file" requirement).
    /// Doesn't go through `snapshot_db_for_destructive_op` because that
    /// takes an `AppState`; we exercise the load-bearing SQL+filesystem
    /// dance directly via the same query the helper runs.
    #[tokio::test]
    async fn vacuum_into_writes_snapshot_and_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let url = format!("sqlite:{}", db_path.display());
        let pool = longbox_db::open(&url).await.unwrap();
        let backup_path = derive_db_backup_path(&url).expect("file URL → backup path");
        assert_eq!(backup_path, tmp.path().join("test.db.pre-delete-backup"));

        // Run the same query the helper runs.
        let stmt = format!("VACUUM INTO '{}'", backup_path.display());
        sqlx::query(&stmt).execute(&pool).await.unwrap();
        assert!(
            backup_path.is_file(),
            "VACUUM INTO must create the backup file"
        );

        // Second run: emulate the "overwrite each delete" guarantee by
        // removing first (as the helper does), then VACUUM INTO again.
        std::fs::remove_file(&backup_path).unwrap();
        sqlx::query(&stmt).execute(&pool).await.unwrap();
        assert!(backup_path.is_file());
    }
}
