//! Library Tidy Step 1 — repo tests for `discovered_folders_repo` and
//! the `series_repo` phantom surface.

mod common;

use common::{fixed_ts, fresh_pool};
use longbox_db::{
    discovered_folders_repo, file_repo, issue_repo, library_root_repo, series_repo, DbError,
    DiscoveredFolder, NewFile, NewIssue, NewLibraryRoot, NewSeries,
};
use sqlx::SqlitePool;

// -------- discovered_folders_repo --------

fn folder(name: &str, file_count: i64) -> DiscoveredFolder {
    DiscoveredFolder {
        folder_name: name.into(),
        file_count,
    }
}

#[tokio::test]
async fn discovered_folder_upsert_inserts_new() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Wolverine (1982)", 24))
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].folder_name, "Wolverine (1982)");
    assert_eq!(rows[0].file_count, 24);
    assert!(rows[0].dismissed_at.is_none());
}

#[tokio::test]
async fn discovered_folder_upsert_refreshes_file_count() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Saga (2012)", 1))
        .await
        .unwrap();
    discovered_folders_repo::upsert(&pool, folder("Saga (2012)", 9))
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1, "re-detection updates, never duplicates");
    assert_eq!(rows[0].file_count, 9);
}

#[tokio::test]
async fn discovered_folder_upsert_skips_a_dismissed_folder() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Bone (1991)", 3))
        .await
        .unwrap();
    discovered_folders_repo::dismiss(&pool, &["Bone (1991)".into()])
        .await
        .unwrap();
    // Re-detected on a later scan — a dismissed folder must not resurface.
    discovered_folders_repo::upsert(&pool, folder("Bone (1991)", 5))
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert!(rows.is_empty(), "a dismissed folder stays dismissed");
}

#[tokio::test]
async fn discovered_folder_list_excludes_dismissed() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("A (2000)", 1))
        .await
        .unwrap();
    discovered_folders_repo::upsert(&pool, folder("B (2001)", 2))
        .await
        .unwrap();
    discovered_folders_repo::dismiss(&pool, &["A (2000)".into()])
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].folder_name, "B (2001)");
}

#[tokio::test]
async fn discovered_folder_dismiss_is_idempotent_and_tolerates_unknowns() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("X (1999)", 1))
        .await
        .unwrap();
    let first = discovered_folders_repo::dismiss(&pool, &["X (1999)".into()])
        .await
        .unwrap();
    assert_eq!(first, 1);
    // Re-dismissing the same folder, plus an unknown one — both no-ops.
    let second = discovered_folders_repo::dismiss(&pool, &["X (1999)".into(), "never-seen".into()])
        .await
        .unwrap();
    assert_eq!(second, 0);
}

// -------- series_repo phantoms --------

async fn insert_series(pool: &SqlitePool, title: &str) -> i64 {
    series_repo::insert(
        pool,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
            start_year: Some(2010),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn update_last_matched_count_sets_the_value() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Saga").await;
    series_repo::update_last_matched_count(&pool, sid, 7)
        .await
        .unwrap();
    // No files -> the series is a phantom; its last_matched_count is now 7.
    let phantoms = series_repo::list_phantoms(&pool).await.unwrap();
    let row = phantoms.iter().find(|p| p.id == sid).expect("phantom row");
    assert_eq!(row.last_matched_count, 7);
}

#[tokio::test]
async fn update_last_matched_count_unknown_series_is_not_found() {
    let pool = fresh_pool().await;
    let err = series_repo::update_last_matched_count(&pool, 9999, 1)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound));
}

#[tokio::test]
async fn list_phantoms_excludes_series_with_an_owned_file() {
    let pool = fresh_pool().await;
    let phantom = insert_series(&pool, "Phantom Series").await;
    let owned = insert_series(&pool, "Owned Series").await;

    // Give `owned` an owned, present file.
    let library_root_id = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        &pool,
        NewIssue {
            series_id: owned,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    file_repo::insert(
        &pool,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id,
            path_relative: "Owned Series/1.cbz".into(),
            size_bytes: 1,
            mtime: fixed_ts(),
            last_scanned_at: fixed_ts(),
            match_method: "comicinfo_xml".into(),
            match_confidence: 0.95,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: fixed_ts(),
            matched_at: Some(fixed_ts()),
        },
    )
    .await
    .unwrap();

    let ids: Vec<i64> = series_repo::list_phantoms(&pool)
        .await
        .unwrap()
        .iter()
        .map(|p| p.id)
        .collect();
    assert!(ids.contains(&phantom), "a zero-owned series is a phantom");
    assert!(
        !ids.contains(&owned),
        "a series with an owned, present file is not a phantom"
    );
}
