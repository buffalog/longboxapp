mod common;

use common::{fixed_ts, fresh_pool};
use longbox_db::{
    file_repo, issue_repo, library_root_repo, series_repo, DbError, FileUpdate, NewFile,
    NewIssue, NewLibraryRoot, NewSeries,
};
use sqlx::SqlitePool;

async fn seed(pool: &SqlitePool) -> (i64, i64, i64) {
    let library_root_id = library_root_repo::insert(
        pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap()
    .id;
    let series_id = series_repo::insert(
        pool,
        NewSeries {
            cv_id: Some(1),
            metron_id: None,
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2012),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        pool,
        NewIssue {
            series_id,
            cv_issue_id: Some(101),
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
    (library_root_id, series_id, issue_id)
}

fn new_file(library_root_id: i64, path: &str, status: &str, issue_id: Option<i64>) -> NewFile {
    NewFile {
        issue_id,
        library_root_id,
        path_relative: path.to_string(),
        size_bytes: 12345,
        mtime: fixed_ts(),
        last_scanned_at: fixed_ts(),
        match_method: if issue_id.is_some() {
            "comicinfo_xml".to_string()
        } else {
            "unmatched".to_string()
        },
        match_confidence: if issue_id.is_some() { 0.95 } else { 0.0 },
        status: status.to_string(),
        cached_comicinfo_xml: None,
        cached_at: None,
        is_present: true,
        last_seen_at: fixed_ts(),
    }
}

#[tokio::test]
async fn insert_and_find_by_id() {
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;
    let row = file_repo::insert(
        &pool,
        new_file(library_root_id, "Saga 1.cbz", "owned", Some(issue_id)),
    )
    .await
    .unwrap();
    assert_eq!(row.path_relative, "Saga 1.cbz");
    assert_eq!(row.status, "owned");
    let found = file_repo::find_by_id(&pool, row.id).await.unwrap();
    assert_eq!(found, Some(row));
}

#[tokio::test]
async fn find_by_path_hot_cache_lookup() {
    let pool = fresh_pool().await;
    let (library_root_id, _, issue_id) = seed(&pool).await;
    let inserted = file_repo::insert(
        &pool,
        new_file(library_root_id, "Saga 1.cbz", "owned", Some(issue_id)),
    )
    .await
    .unwrap();
    let found = file_repo::find_by_path(&pool, library_root_id, "Saga 1.cbz")
        .await
        .unwrap();
    assert_eq!(found.unwrap().id, inserted.id);

    let missing = file_repo::find_by_path(&pool, library_root_id, "Nope.cbz")
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn list_by_library_root() {
    let pool = fresh_pool().await;
    let (library_root_id, _, issue_id) = seed(&pool).await;
    file_repo::insert(
        &pool,
        new_file(library_root_id, "a.cbz", "owned", Some(issue_id)),
    )
    .await
    .unwrap();
    file_repo::insert(
        &pool,
        new_file(library_root_id, "b.cbz", "unmatched", None),
    )
    .await
    .unwrap();
    let rows = file_repo::list_by_library_root(&pool, library_root_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn list_unmatched_for_series_filters_by_status_and_root() {
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;
    // A second library_root to verify scoping.
    let other_root_id = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/other".into(),
        },
    )
    .await
    .unwrap()
    .id;

    file_repo::insert(
        &pool,
        new_file(library_root_id, "owned.cbz", "owned", Some(issue_id)),
    )
    .await
    .unwrap();
    file_repo::insert(
        &pool,
        new_file(library_root_id, "needs_review.cbz", "needs_review", None),
    )
    .await
    .unwrap();
    file_repo::insert(
        &pool,
        new_file(library_root_id, "unmatched1.cbz", "unmatched", None),
    )
    .await
    .unwrap();
    file_repo::insert(
        &pool,
        new_file(library_root_id, "unmatched2.cbz", "unmatched", None),
    )
    .await
    .unwrap();
    file_repo::insert(
        &pool,
        new_file(library_root_id, "ignored.cbz", "ignored", None),
    )
    .await
    .unwrap();
    // Unmatched in OTHER root — must not appear in the filtered list.
    file_repo::insert(
        &pool,
        new_file(other_root_id, "other-unmatched.cbz", "unmatched", None),
    )
    .await
    .unwrap();

    let rows = file_repo::list_unmatched_for_series(&pool, library_root_id)
        .await
        .unwrap();
    let paths: Vec<&str> = rows.iter().map(|r| r.path_relative.as_str()).collect();
    assert_eq!(paths, vec!["unmatched1.cbz", "unmatched2.cbz"]);
}

#[tokio::test]
async fn list_by_status_returns_only_matching() {
    let pool = fresh_pool().await;
    let (library_root_id, _, issue_id) = seed(&pool).await;
    file_repo::insert(
        &pool,
        new_file(library_root_id, "a.cbz", "owned", Some(issue_id)),
    )
    .await
    .unwrap();
    file_repo::insert(
        &pool,
        new_file(library_root_id, "b.cbz", "unmatched", None),
    )
    .await
    .unwrap();
    let rows = file_repo::list_by_status(&pool, library_root_id, "owned")
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path_relative, "a.cbz");
}

#[tokio::test]
async fn update_can_set_issue_id_to_null_for_ignore_flow() {
    let pool = fresh_pool().await;
    let (library_root_id, _, issue_id) = seed(&pool).await;
    let row = file_repo::insert(
        &pool,
        new_file(library_root_id, "Saga 1.cbz", "owned", Some(issue_id)),
    )
    .await
    .unwrap();

    let updated = file_repo::update(
        &pool,
        row.id,
        FileUpdate {
            issue_id: None,
            size_bytes: row.size_bytes,
            mtime: row.mtime,
            last_scanned_at: row.last_scanned_at,
            match_method: "ignored".into(),
            match_confidence: 0.0,
            status: "ignored".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: row.is_present,
            last_seen_at: row.last_seen_at,
        },
    )
    .await
    .unwrap();
    assert!(updated.issue_id.is_none());
    assert_eq!(updated.status, "ignored");
}

#[tokio::test]
async fn delete_removes_row() {
    let pool = fresh_pool().await;
    let (library_root_id, _, _) = seed(&pool).await;
    let row = file_repo::insert(
        &pool,
        new_file(library_root_id, "a.cbz", "unmatched", None),
    )
    .await
    .unwrap();
    file_repo::delete(&pool, row.id).await.unwrap();
    assert!(file_repo::find_by_id(&pool, row.id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_missing_returns_not_found() {
    let pool = fresh_pool().await;
    let err = file_repo::delete(&pool, 999).await.unwrap_err();
    assert!(matches!(err, DbError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn duplicate_path_in_root_surfaces_unique_violation() {
    let pool = fresh_pool().await;
    let (library_root_id, _, _) = seed(&pool).await;
    file_repo::insert(
        &pool,
        new_file(library_root_id, "Saga 1.cbz", "unmatched", None),
    )
    .await
    .unwrap();
    let err = file_repo::insert(
        &pool,
        new_file(library_root_id, "Saga 1.cbz", "unmatched", None),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            DbError::UniqueViolation {
                field: "files_library_root_id_path_relative"
            }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn deleting_issue_clears_issue_id_on_files() {
    // FK: ON DELETE SET NULL for files.issue_id.
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;
    let row = file_repo::insert(
        &pool,
        new_file(library_root_id, "a.cbz", "owned", Some(issue_id)),
    )
    .await
    .unwrap();

    sqlx::query!(r#"DELETE FROM issues WHERE id = ?"#, issue_id)
        .execute(&pool)
        .await
        .unwrap();

    let after = file_repo::find_by_id(&pool, row.id).await.unwrap().unwrap();
    assert!(after.issue_id.is_none());
}

#[tokio::test]
async fn insert_defaults_is_present_true() {
    let pool = fresh_pool().await;
    let (library_root_id, _, _) = seed(&pool).await;
    let row = file_repo::insert(
        &pool,
        new_file(library_root_id, "a.cbz", "unmatched", None),
    )
    .await
    .unwrap();
    assert!(row.is_present);
}

#[tokio::test]
async fn mark_files_not_seen_since_flips_old_rows() {
    use time::macros::datetime;

    let pool = fresh_pool().await;
    let (library_root_id, _, _) = seed(&pool).await;

    // Two rows last_seen well in the past.
    let old = datetime!(2024-01-01 00:00:00);
    let mut a = new_file(library_root_id, "old-1.cbz", "unmatched", None);
    a.last_seen_at = old;
    let mut b = new_file(library_root_id, "old-2.cbz", "unmatched", None);
    b.last_seen_at = old;
    file_repo::insert(&pool, a).await.unwrap();
    file_repo::insert(&pool, b).await.unwrap();

    // One row last_seen "now" (after cutoff).
    let fresh = datetime!(2030-01-01 00:00:00);
    let mut c = new_file(library_root_id, "fresh.cbz", "unmatched", None);
    c.last_seen_at = fresh;
    file_repo::insert(&pool, c).await.unwrap();

    let cutoff = datetime!(2026-06-01 00:00:00);
    let affected = file_repo::mark_files_not_seen_since(&pool, library_root_id, cutoff)
        .await
        .unwrap();
    assert_eq!(affected, 2, "two old rows should be flipped");

    let all = file_repo::list_by_library_root(&pool, library_root_id)
        .await
        .unwrap();
    let presence_by_path: std::collections::HashMap<&str, bool> = all
        .iter()
        .map(|r| (r.path_relative.as_str(), r.is_present))
        .collect();
    assert_eq!(presence_by_path.get("old-1.cbz"), Some(&false));
    assert_eq!(presence_by_path.get("old-2.cbz"), Some(&false));
    assert_eq!(presence_by_path.get("fresh.cbz"), Some(&true));
}

#[tokio::test]
async fn mark_files_not_seen_since_is_scoped_to_library_root() {
    use time::macros::datetime;

    let pool = fresh_pool().await;
    let (library_root_id, _, _) = seed(&pool).await;
    let other_root_id = library_root_repo::insert(
        &pool,
        longbox_db::NewLibraryRoot {
            path: "/other".into(),
        },
    )
    .await
    .unwrap()
    .id;

    let old = datetime!(2024-01-01 00:00:00);
    let mut a = new_file(library_root_id, "a.cbz", "unmatched", None);
    a.last_seen_at = old;
    let mut b = new_file(other_root_id, "b.cbz", "unmatched", None);
    b.last_seen_at = old;
    file_repo::insert(&pool, a).await.unwrap();
    file_repo::insert(&pool, b).await.unwrap();

    let cutoff = datetime!(2026-06-01 00:00:00);
    let affected = file_repo::mark_files_not_seen_since(&pool, library_root_id, cutoff)
        .await
        .unwrap();
    assert_eq!(affected, 1);

    let other_row = file_repo::find_by_path(&pool, other_root_id, "b.cbz")
        .await
        .unwrap()
        .unwrap();
    assert!(other_row.is_present, "other library root's row must remain untouched");
}
