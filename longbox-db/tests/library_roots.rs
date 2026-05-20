mod common;

use common::fresh_pool;
use longbox_db::{library_root_repo, DbError, NewLibraryRoot};

#[tokio::test]
async fn insert_and_find_by_id() {
    let pool = fresh_pool().await;
    let inserted = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/comics".to_string(),
        },
    )
    .await
    .unwrap();
    assert_eq!(inserted.path, "/comics");
    assert!(inserted.id > 0);

    let found = library_root_repo::find_by_id(&pool, inserted.id)
        .await
        .unwrap();
    assert_eq!(found, Some(inserted));
}

#[tokio::test]
async fn find_by_id_returns_none_for_missing() {
    let pool = fresh_pool().await;
    let r = library_root_repo::find_by_id(&pool, 999).await.unwrap();
    assert!(r.is_none());
}

#[tokio::test]
async fn list_all_orders_by_id() {
    let pool = fresh_pool().await;
    library_root_repo::insert(&pool, NewLibraryRoot { path: "/a".into() })
        .await
        .unwrap();
    library_root_repo::insert(&pool, NewLibraryRoot { path: "/b".into() })
        .await
        .unwrap();
    let rows = library_root_repo::list_all(&pool).await.unwrap();
    let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, vec!["/a", "/b"]);
}

#[tokio::test]
async fn update_path_changes_value() {
    let pool = fresh_pool().await;
    let row = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/old".into(),
        },
    )
    .await
    .unwrap();
    library_root_repo::update_path(&pool, row.id, "/new")
        .await
        .unwrap();
    let updated = library_root_repo::find_by_id(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.path, "/new");
}

#[tokio::test]
async fn update_path_on_missing_returns_not_found() {
    let pool = fresh_pool().await;
    let err = library_root_repo::update_path(&pool, 999, "/x")
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn duplicate_path_surfaces_unique_violation() {
    let pool = fresh_pool().await;
    library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap();
    let err = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, DbError::UniqueViolation { field: "path" }),
        "got {err:?}"
    );
}
