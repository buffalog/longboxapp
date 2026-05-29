//! Shared test helpers for the enrichment integration suite.

use longbox_db::{
    issue_repo, library_root_repo, series_repo, NewIssue, NewLibraryRoot, NewSeries, Pool,
};

/// Open a fresh in-memory DB with all migrations applied. Keeps
/// each test isolated.
pub async fn fresh_pool() -> Pool {
    longbox_db::open(":memory:")
        .await
        .expect("open :memory: db")
}

/// Seed a shallow series + N synthesized issues + one library root.
/// Returns the series_id.
#[allow(dead_code)]
pub async fn seed_shallow_series_with_issues(
    pool: &Pool,
    title: &str,
    start_year: Option<i32>,
    issue_numbers: &[&str],
) -> i64 {
    let series_id = series_repo::insert(
        pool,
        NewSeries {
            cv_id: None,
            metron_id: None,
            sort_title: longbox_core::normalize_title(title),
            title: title.into(),
            start_year,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    for n in issue_numbers {
        issue_repo::insert(
            pool,
            NewIssue {
                series_id,
                cv_issue_id: None,
                metron_issue_id: None,
                number: (*n).into(),
                title: None,
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }
    series_id
}

#[allow(dead_code)]
pub async fn ensure_library_root(pool: &Pool) -> i64 {
    library_root_repo::insert(
        pool,
        NewLibraryRoot {
            path: "/tmp/cv-enrichment-test".into(),
        },
    )
    .await
    .unwrap()
    .id
}
