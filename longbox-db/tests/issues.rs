mod common;

use std::time::Instant;

use common::fresh_pool;
use longbox_db::{issue_repo, series_repo, DbError, IssueUpdate, NewIssue, NewSeries};

async fn seed_series(pool: &sqlx::SqlitePool) -> i64 {
    series_repo::insert(
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
    .id
}

fn new_issue(series_id: i64, number: &str, cv: Option<i64>) -> NewIssue {
    NewIssue {
        series_id,
        cv_issue_id: cv,
        metron_issue_id: None,
        number: number.to_string(),
        title: None,
        cover_date: None,
        summary: None,
        cover_url: None,
    }
}

#[tokio::test]
async fn insert_and_find_by_id() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::insert(&pool, new_issue(series_id, "1", Some(100)))
        .await
        .unwrap();
    assert_eq!(row.number, "1");
    let found = issue_repo::find_by_id(&pool, row.id).await.unwrap();
    assert_eq!(found, Some(row));
}

#[tokio::test]
async fn find_by_cv_issue_id() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::insert(&pool, new_issue(series_id, "1", Some(42)))
        .await
        .unwrap();
    let found = issue_repo::find_by_cv_issue_id(&pool, 42).await.unwrap();
    assert_eq!(found.unwrap().id, row.id);
}

#[tokio::test]
async fn find_by_metron_issue_id() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let input = NewIssue {
        metron_issue_id: Some("saga-1-2012".to_string()),
        ..new_issue(series_id, "1", None)
    };
    let row = issue_repo::insert(&pool, input).await.unwrap();
    let found = issue_repo::find_by_metron_issue_id(&pool, "saga-1-2012")
        .await
        .unwrap();
    assert_eq!(found.unwrap().id, row.id);
}

#[tokio::test]
async fn list_by_series() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    issue_repo::insert(&pool, new_issue(series_id, "2", Some(2)))
        .await
        .unwrap();
    let rows = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    let numbers: Vec<&str> = rows.iter().map(|r| r.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2"]);
}

#[tokio::test]
async fn update_overwrites_metadata_fields() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let row = issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    let updated = issue_repo::update(
        &pool,
        row.id,
        IssueUpdate {
            title: Some("One Small Step".into()),
            cover_date: Some("2012-03-14".into()),
            summary: Some("A summary.".into()),
            cover_url: Some("https://example.com/saga-1.jpg".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.title.as_deref(), Some("One Small Step"));
    assert_eq!(updated.cover_date.as_deref(), Some("2012-03-14"));
}

#[tokio::test]
async fn update_missing_returns_not_found() {
    let pool = fresh_pool().await;
    let err = issue_repo::update(
        &pool,
        999,
        IssueUpdate {
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DbError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn bulk_insert_returns_rows_in_input_order() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let inputs: Vec<NewIssue> = (1..=5)
        .map(|n| new_issue(series_id, &n.to_string(), Some(1000 + i64::from(n))))
        .collect();
    let rows = issue_repo::bulk_insert(&pool, inputs).await.unwrap();
    let numbers: Vec<&str> = rows.iter().map(|r| r.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2", "3", "4", "5"]);
}

#[tokio::test]
async fn bulk_insert_500_in_under_100ms() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    let inputs: Vec<NewIssue> = (1..=500)
        .map(|n| new_issue(series_id, &n.to_string(), Some(10_000 + i64::from(n))))
        .collect();
    let start = Instant::now();
    let rows = issue_repo::bulk_insert(&pool, inputs).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(rows.len(), 500);
    assert!(
        elapsed.as_millis() < 100,
        "bulk_insert 500 took {elapsed:?}, expected < 100ms"
    );
}

#[tokio::test]
async fn bulk_insert_empty_returns_empty() {
    let pool = fresh_pool().await;
    let rows = issue_repo::bulk_insert(&pool, vec![]).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn duplicate_series_number_surfaces_unique_violation() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    let err = issue_repo::insert(&pool, new_issue(series_id, "1", Some(2)))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            DbError::UniqueViolation {
                field: "issues_series_id_number"
            }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn duplicate_cv_issue_id_surfaces_unique_violation() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(99)))
        .await
        .unwrap();
    let err = issue_repo::insert(&pool, new_issue(series_id, "2", Some(99)))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            DbError::UniqueViolation {
                field: "cv_issue_id"
            }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn cascade_delete_when_series_dropped() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool).await;
    issue_repo::insert(&pool, new_issue(series_id, "1", Some(1)))
        .await
        .unwrap();
    sqlx::query!(r#"DELETE FROM series WHERE id = ?"#, series_id)
        .execute(&pool)
        .await
        .unwrap();
    let rows = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    assert!(rows.is_empty());
}
