mod common;

use common::fresh_pool;
use longbox_db::{parsing_pattern_repo, DbError, NewParsingPattern};

#[tokio::test]
async fn list_enabled_returns_seeds_in_priority_order() {
    // The initial four (5/10/20/30) plus the three the A.9 parser
    // hot-fix added (11/12/15).
    let pool = fresh_pool().await;
    let rows = parsing_pattern_repo::list_enabled(&pool).await.unwrap();
    let priorities: Vec<i64> = rows.iter().map(|r| r.priority).collect();
    assert_eq!(priorities, vec![5, 10, 11, 12, 15, 20, 30]);
    assert!(rows.iter().all(|r| r.enabled));
}

#[tokio::test]
async fn set_enabled_false_removes_from_enabled_list() {
    let pool = fresh_pool().await;
    // Pattern id 1 is the volume-aware seed.
    parsing_pattern_repo::set_enabled(&pool, 1, false)
        .await
        .unwrap();
    let rows = parsing_pattern_repo::list_enabled(&pool).await.unwrap();
    let priorities: Vec<i64> = rows.iter().map(|r| r.priority).collect();
    assert_eq!(priorities, vec![10, 11, 12, 15, 20, 30]);
}

#[tokio::test]
async fn insert_new_pattern() {
    let pool = fresh_pool().await;
    let row = parsing_pattern_repo::insert(
        &pool,
        NewParsingPattern {
            name: "Custom".into(),
            pattern: r"^(?P<series>.+)\.(?i:cbz)$".into(),
            priority: 100,
            enabled: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(row.priority, 100);
    assert!(row.enabled);
    assert_eq!(row.name, "Custom");
}

#[tokio::test]
async fn find_by_id_for_seed_row() {
    let pool = fresh_pool().await;
    let row = parsing_pattern_repo::find_by_id(&pool, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.priority, 5);
    assert_eq!(row.name, "Series Vol N #M");
}

#[tokio::test]
async fn update_changes_fields() {
    let pool = fresh_pool().await;
    let updated = parsing_pattern_repo::update(
        &pool,
        1,
        NewParsingPattern {
            name: "Renamed".into(),
            pattern: r"^.+$".into(),
            priority: 1,
            enabled: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.priority, 1);
    assert!(!updated.enabled);
}

#[tokio::test]
async fn update_missing_returns_not_found() {
    let pool = fresh_pool().await;
    let err = parsing_pattern_repo::update(
        &pool,
        999,
        NewParsingPattern {
            name: "X".into(),
            pattern: "X".into(),
            priority: 1,
            enabled: true,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DbError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn list_all_includes_disabled() {
    let pool = fresh_pool().await;
    parsing_pattern_repo::set_enabled(&pool, 1, false)
        .await
        .unwrap();
    let all = parsing_pattern_repo::list_all(&pool).await.unwrap();
    assert_eq!(all.len(), 7);
}
