use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::library_root_repo;
use serde::Serialize;
use time::OffsetDateTime;
use tracing::error;
use ulid::Ulid;

use crate::error::ApiError;
use crate::state::{AppState, CurrentScan, ScanKind};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/library-roots/:id/scan", post(start_full))
        .route(
            "/library-roots/:id/rescan-unmatched",
            post(start_rescan_unmatched),
        )
        .route("/scans/current", get(current))
        .route("/scans/recent", get(recent))
}

#[derive(Debug, Serialize)]
struct StartScanResponse {
    scan_id: String,
    status: &'static str,
}

async fn start_full(
    State(state): State<AppState>,
    Path(library_root_id): Path<i64>,
) -> Result<(StatusCode, Json<StartScanResponse>), ApiError> {
    start_scan(state, library_root_id, ScanKind::Full).await
}

async fn start_rescan_unmatched(
    State(state): State<AppState>,
    Path(library_root_id): Path<i64>,
) -> Result<(StatusCode, Json<StartScanResponse>), ApiError> {
    start_scan(state, library_root_id, ScanKind::RescanUnmatched).await
}

async fn start_scan(
    state: AppState,
    library_root_id: i64,
    kind: ScanKind,
) -> Result<(StatusCode, Json<StartScanResponse>), ApiError> {
    // 1. Library root must exist.
    if library_root_repo::find_by_id(&state.db, library_root_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound {
            resource: "library_root",
            id: library_root_id.to_string(),
        });
    }

    // 2 + 3 + 4. Acquire scan_status write lock; reject if busy.
    let scan_id = Ulid::new().to_string();
    {
        let mut status = state.scan_status.write().await;
        if status.current.is_some() {
            return Err(ApiError::Conflict {
                code: "conflict.scan_running",
                message: "Scan already in progress".into(),
            });
        }
        status.current = Some(CurrentScan {
            scan_id: scan_id.clone(),
            library_root_id,
            kind,
            started_at: OffsetDateTime::now_utc(),
        });
    }

    // 5 + 6. Spawn the actual scan. The handler returns immediately.
    let scanner = state.scanner.clone();
    let status_lock = state.scan_status.clone();
    tokio::spawn(async move {
        let result = match kind {
            ScanKind::Full => scanner.scan_full(library_root_id).await,
            ScanKind::RescanUnmatched => scanner.rescan_unmatched(library_root_id).await,
            // RematchForSeries is not exposed via this endpoint path.
            ScanKind::RematchForSeries => unreachable!("invalid kind for HTTP-triggered scan"),
        };
        let mut status = status_lock.write().await;
        status.current = None;
        match result {
            Ok(report) => status.record(report),
            Err(e) => error!(target: "longbox_web", err = %e, "background scan failed"),
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(StartScanResponse {
            scan_id,
            status: "started",
        }),
    ))
}

async fn current(
    State(state): State<AppState>,
) -> Json<Option<CurrentScan>> {
    Json(state.scan_status.read().await.current.clone())
}

async fn recent(
    State(state): State<AppState>,
) -> Json<Vec<longbox_scanner::ScanReport>> {
    Json(state.scan_status.read().await.recent.iter().cloned().collect())
}
