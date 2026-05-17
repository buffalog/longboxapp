mod common;

use common::fresh_pool;
use longbox_db::{
    library_root_repo, scan_run_repo, DbError, NewLibraryRoot, NewScanRun, ScanCompletion,
    ScanProgress, ScanRunKind,
};

async fn seed_root(pool: &sqlx::SqlitePool) -> i64 {
    library_root_repo::insert(
        pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn insert_starts_running() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let row = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
        .await
        .unwrap();
    assert_eq!(row.status, "running");
    assert!(row.finished_at.is_none());
    assert_eq!(row.files_seen, 0);
}

#[tokio::test]
async fn update_progress_idempotent() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let row = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
        .await
        .unwrap();
    let progress = ScanProgress {
        files_seen: 100,
        files_added: 25,
        files_updated: 5,
        files_matched: 20,
        files_needs_review: 7,
        files_unmatched: 3,
    };
    scan_run_repo::update_progress(&pool, row.id, progress)
        .await
        .unwrap();
    let after_first = scan_run_repo::find_by_id(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    scan_run_repo::update_progress(&pool, row.id, progress)
        .await
        .unwrap();
    let after_second = scan_run_repo::find_by_id(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_first, after_second);
    assert_eq!(after_first.files_seen, 100);
    assert_eq!(after_first.files_matched, 20);
}

#[tokio::test]
async fn complete_transitions_status_and_sets_finished_at() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let row = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
        .await
        .unwrap();
    scan_run_repo::complete(&pool, row.id).await.unwrap();
    let done = scan_run_repo::find_by_id(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "completed");
    assert!(done.finished_at.is_some());
}

#[tokio::test]
async fn fail_records_error_message() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let row = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
        .await
        .unwrap();
    scan_run_repo::fail(&pool, row.id, "permission denied at /comics/X")
        .await
        .unwrap();
    let done = scan_run_repo::find_by_id(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "failed");
    assert_eq!(
        done.error_message.as_deref(),
        Some("permission denied at /comics/X")
    );
    assert!(done.finished_at.is_some());
}

#[tokio::test]
async fn list_recent_orders_descending() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let a = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
        .await
        .unwrap();
    let b = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
        .await
        .unwrap();
    let c = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
        .await
        .unwrap();
    let rows = scan_run_repo::list_recent(&pool, 20, &[]).await.unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![c.id, b.id, a.id]);
}

#[tokio::test]
async fn complete_missing_returns_not_found() {
    let pool = fresh_pool().await;
    let err = scan_run_repo::complete(&pool, 999).await.unwrap_err();
    assert!(matches!(err, DbError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn list_recent_filters_by_kind() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let full = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
    .await
    .unwrap();
    let rescan = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::RescanUnmatched,
        },
    )
    .await
    .unwrap();
    let rematch = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::RematchForSeries,
        },
    )
    .await
    .unwrap();

    let only_full = scan_run_repo::list_recent(&pool, 20, &[ScanRunKind::Full])
        .await
        .unwrap();
    assert_eq!(only_full.len(), 1);
    assert_eq!(only_full[0].id, full.id);

    let user_triggered = scan_run_repo::list_recent(
        &pool,
        20,
        &[ScanRunKind::Full, ScanRunKind::RescanUnmatched],
    )
    .await
    .unwrap();
    assert_eq!(user_triggered.len(), 2);
    let ids: Vec<i64> = user_triggered.iter().map(|r| r.id).collect();
    assert!(ids.contains(&full.id));
    assert!(ids.contains(&rescan.id));
    assert!(!ids.contains(&rematch.id));
}

#[tokio::test]
async fn complete_with_stats_writes_counters_and_status() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let row = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
    .await
    .unwrap();
    scan_run_repo::complete_with_stats(
        &pool,
        row.id,
        ScanCompletion {
            files_seen: 1500,
            files_added: 1500,
            files_updated: 0,
            files_matched: 950,
            files_needs_review: 50,
            files_unmatched: 500,
        },
    )
    .await
    .unwrap();
    let done = scan_run_repo::find_by_id(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "completed");
    assert_eq!(done.files_seen, 1500);
    assert_eq!(done.files_matched, 950);
    assert_eq!(done.kind, "full");
    assert!(done.finished_at.is_some());
}

#[tokio::test]
async fn mark_running_as_failed_sweeps_only_running_rows() {
    let pool = fresh_pool().await;
    let library_root_id = seed_root(&pool).await;
    let running = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
    .await
    .unwrap();
    let done = scan_run_repo::insert(
        &pool,
        NewScanRun {
            library_root_id,
            kind: ScanRunKind::Full,
        },
    )
    .await
    .unwrap();
    scan_run_repo::complete(&pool, done.id).await.unwrap();

    let swept = scan_run_repo::mark_running_as_failed(&pool, "interrupted by restart")
        .await
        .unwrap();
    assert_eq!(swept, 1);
    let r = scan_run_repo::find_by_id(&pool, running.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, "failed");
    assert_eq!(r.error_message.as_deref(), Some("interrupted by restart"));
    let d = scan_run_repo::find_by_id(&pool, done.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.status, "completed");
}
