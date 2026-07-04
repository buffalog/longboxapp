mod common;

use common::fresh_pool;
use longbox_db::{issue_repo, reading_progress_repo, series_repo, NewIssue, NewSeries};

async fn seed_issue(pool: &sqlx::SqlitePool) -> i64 {
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
    issue_repo::insert(
        pool,
        NewIssue {
            series_id,
            cv_issue_id: Some(100),
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
    .id
}

#[tokio::test]
async fn absent_row_reads_as_page_one() {
    let pool = fresh_pool().await;
    let issue_id = seed_issue(&pool).await;
    assert_eq!(
        reading_progress_repo::get_last_page(&pool, issue_id)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn set_then_get_round_trips_and_upserts() {
    let pool = fresh_pool().await;
    let issue_id = seed_issue(&pool).await;

    reading_progress_repo::set_last_page(&pool, issue_id, 7)
        .await
        .unwrap();
    assert_eq!(
        reading_progress_repo::get_last_page(&pool, issue_id)
            .await
            .unwrap(),
        7
    );

    // Second write updates the existing row rather than erroring on the PK.
    reading_progress_repo::set_last_page(&pool, issue_id, 12)
        .await
        .unwrap();
    assert_eq!(
        reading_progress_repo::get_last_page(&pool, issue_id)
            .await
            .unwrap(),
        12
    );
}

#[tokio::test]
async fn progress_cascades_on_issue_delete() {
    let pool = fresh_pool().await;
    let issue_id = seed_issue(&pool).await;
    reading_progress_repo::set_last_page(&pool, issue_id, 5)
        .await
        .unwrap();

    sqlx::query!("DELETE FROM issues WHERE id = ?", issue_id)
        .execute(&pool)
        .await
        .unwrap();

    // Row is gone (cascade), so it reads back as the page-1 default.
    assert_eq!(
        reading_progress_repo::get_last_page(&pool, issue_id)
            .await
            .unwrap(),
        1
    );
}
