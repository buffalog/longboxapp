mod common;

use common::fresh_pool;
use longbox_db::{issue_repo, series_repo, NewIssue, NewSeries};

fn series_input() -> NewSeries {
    NewSeries {
        cv_id: Some(42),
        metron_id: None,
        title: "Saga".into(),
        sort_title: "saga".into(),
        start_year: Some(2012),
        publisher: None,
        description: None,
        cover_url: None,
    }
}

#[tokio::test]
async fn rollback_leaves_no_rows() {
    let pool = fresh_pool().await;
    let mut tx = pool.begin().await.unwrap();

    let series = series_repo::insert(&mut *tx, series_input()).await.unwrap();
    issue_repo::insert(
        &mut *tx,
        NewIssue {
            series_id: series.id,
            cv_issue_id: Some(1),
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // Bail without committing.
    drop(tx);

    let series_after = series_repo::list_all(&pool).await.unwrap();
    assert!(series_after.is_empty(), "series rollback should leave none");
}

#[tokio::test]
async fn commit_persists_rows() {
    let pool = fresh_pool().await;
    let mut tx = pool.begin().await.unwrap();

    let series = series_repo::insert(&mut *tx, series_input()).await.unwrap();
    let issue = issue_repo::insert(
        &mut *tx,
        NewIssue {
            series_id: series.id,
            cv_issue_id: Some(1),
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    tx.commit().await.unwrap();

    let series_after = series_repo::find_by_id(&pool, series.id).await.unwrap();
    let issue_after = issue_repo::find_by_id(&pool, issue.id).await.unwrap();
    assert!(series_after.is_some());
    assert!(issue_after.is_some());
}

#[tokio::test]
async fn explicit_rollback_leaves_no_rows() {
    let pool = fresh_pool().await;
    let mut tx = pool.begin().await.unwrap();

    series_repo::insert(&mut *tx, series_input()).await.unwrap();
    tx.rollback().await.unwrap();

    let after = series_repo::list_all(&pool).await.unwrap();
    assert!(after.is_empty());
}
