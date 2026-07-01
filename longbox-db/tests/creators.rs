mod common;
use common::{fixed_ts, fresh_pool};
use longbox_comicvine::CvPersonCredit;
use longbox_db::{
    creator_repo, file_repo, issue_repo, library_root_repo, series_repo, NewFile, NewIssue,
    NewLibraryRoot, NewSeries,
};

async fn seed_owned_issue(pool: &sqlx::SqlitePool, cv_issue_id: i64) -> i64 {
    let root = library_root_repo::insert(
        pool,
        NewLibraryRoot {
            path: format!("/c{cv_issue_id}"),
        },
    )
    .await
    .unwrap();
    let sid = series_repo::insert(
        pool,
        NewSeries {
            cv_id: Some(cv_issue_id * 10),
            metron_id: None,
            title: "Deadly Class".into(),
            sort_title: "deadly class".into(),
            start_year: Some(2014),
            publisher: Some("Image".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let iid = issue_repo::insert(
        pool,
        NewIssue {
            series_id: sid,
            cv_issue_id: Some(cv_issue_id),
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: Some("2014-01-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    file_repo::insert(
        pool,
        NewFile {
            issue_id: Some(iid),
            library_root_id: root.id,
            path_relative: format!("d{cv_issue_id}.cbz"),
            size_bytes: 12345,
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
    iid
}

#[tokio::test]
async fn insert_issue_credits_dedupes_creator_and_sets_fetched() {
    let pool = fresh_pool().await;
    let iid = seed_owned_issue(&pool, 1001).await;
    let credits = vec![
        CvPersonCredit {
            cv_person_id: 97470,
            name: "Bob Quinn".into(),
            role: "artist".into(),
        },
        CvPersonCredit {
            cv_person_id: 97470,
            name: "Bob Quinn".into(),
            role: "cover".into(),
        },
        CvPersonCredit {
            cv_person_id: 55,
            name: "Rick Remender".into(),
            role: "writer".into(),
        },
    ];
    creator_repo::insert_issue_credits(&pool, iid, &credits)
        .await
        .unwrap();

    // One creator per cv_person_id (Bob Quinn appears once despite 2 roles).
    let n_creators: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM creators")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n_creators, 2);
    // Three atomic credit rows.
    let n_credits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_credits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n_credits, 3);
    // Idempotent re-insert: no new rows, no error.
    creator_repo::insert_issue_credits(&pool, iid, &credits)
        .await
        .unwrap();
    let n2: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_credits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n2, 3);
    // credits_fetched flipped.
    let fetched: bool = sqlx::query_scalar("SELECT credits_fetched FROM issues WHERE id=?")
        .bind(iid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(fetched);
}

#[tokio::test]
async fn list_issues_needing_credits_filters_owned_unfetched_with_cv_id() {
    let pool = fresh_pool().await;
    let owned = seed_owned_issue(&pool, 2001).await; // owned, cv_id, not fetched -> included
    let already = seed_owned_issue(&pool, 2002).await;
    creator_repo::insert_issue_credits(&pool, already, &[])
        .await
        .unwrap(); // fetched -> excluded
    // owned but NO cv_issue_id -> excluded
    let sid = series_repo::insert(
        &pool,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "X".into(),
            sort_title: "x".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let no_cv = issue_repo::insert(
        &pool,
        NewIssue {
            series_id: sid,
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
    let root = library_root_repo::insert(&pool, NewLibraryRoot { path: "/nocv".into() })
        .await
        .unwrap();
    file_repo::insert(
        &pool,
        NewFile {
            issue_id: Some(no_cv),
            library_root_id: root.id,
            path_relative: "n.cbz".into(),
            size_bytes: 12345,
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

    let work = creator_repo::list_issues_needing_credits(&pool, 50).await.unwrap();
    let ids: Vec<i64> = work.iter().map(|w| w.issue_id).collect();
    assert_eq!(ids, vec![owned], "only the owned, cv-keyed, unfetched issue");
    assert_eq!(work[0].cv_issue_id, 2001);
}

#[tokio::test]
async fn insert_empty_credits_marks_fetched_with_no_rows() {
    let pool = fresh_pool().await;
    let iid = seed_owned_issue(&pool, 1002).await;
    creator_repo::insert_issue_credits(&pool, iid, &[])
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_credits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let fetched: bool = sqlx::query_scalar("SELECT credits_fetched FROM issues WHERE id=?")
        .bind(iid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(fetched, "empty credits (CV NotFound case) must still mark the issue done");
}
