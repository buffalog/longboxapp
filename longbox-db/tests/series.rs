mod common;

use common::{fixed_ts, fresh_pool};
use longbox_db::{
    file_repo, issue_repo, library_root_repo, series_repo, DbError, NewFile, NewIssue,
    NewLibraryRoot, NewSeries, SeriesUpdate,
};

fn walking_dead() -> NewSeries {
    NewSeries {
        cv_id: Some(12345),
        metron_id: None,
        title: "The Walking Dead".to_string(),
        sort_title: "walking dead".to_string(),
        start_year: Some(2003),
        publisher: Some("Image Comics".to_string()),
        description: None,
        cover_url: None,
    }
}

#[tokio::test]
async fn insert_and_find_by_id() {
    let pool = fresh_pool().await;
    let row = series_repo::insert(&pool, walking_dead()).await.unwrap();
    assert!(row.id > 0);
    assert_eq!(row.title, "The Walking Dead");
    assert_eq!(row.sort_title, "walking dead");
    assert_eq!(row.cv_id, Some(12345));
    assert_eq!(row.start_year, Some(2003));

    let found = series_repo::find_by_id(&pool, row.id).await.unwrap();
    assert_eq!(found, Some(row));
}

#[tokio::test]
async fn find_by_cv_id() {
    let pool = fresh_pool().await;
    let row = series_repo::insert(&pool, walking_dead()).await.unwrap();
    let found = series_repo::find_by_cv_id(&pool, 12345).await.unwrap();
    assert_eq!(found.unwrap().id, row.id);
}

#[tokio::test]
async fn find_by_metron_id() {
    let pool = fresh_pool().await;
    let new = NewSeries {
        metron_id: Some("walking-dead-2003".to_string()),
        ..walking_dead()
    };
    let row = series_repo::insert(&pool, new).await.unwrap();
    let found = series_repo::find_by_metron_id(&pool, "walking-dead-2003")
        .await
        .unwrap();
    assert_eq!(found.unwrap().id, row.id);
}

#[tokio::test]
async fn list_all_orders_by_sort_title() {
    let pool = fresh_pool().await;
    series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(1),
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2012),
            ..walking_dead()
        },
    )
    .await
    .unwrap();
    series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(2),
            title: "Action Comics".into(),
            sort_title: "action comics".into(),
            ..walking_dead()
        },
    )
    .await
    .unwrap();
    let rows = series_repo::find_all(&pool).await.unwrap();
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["Action Comics", "Saga"]);
}

#[tokio::test]
async fn update_overwrites_mutable_fields_but_keeps_cv_id() {
    let pool = fresh_pool().await;
    let row = series_repo::insert(&pool, walking_dead()).await.unwrap();
    let updated = series_repo::update(
        &pool,
        row.id,
        SeriesUpdate {
            title: "Walking Dead (Refreshed)".into(),
            sort_title: "walking dead refreshed".into(),
            start_year: Some(2004),
            publisher: Some("Skybound".into()),
            description: Some("Apocalypse comic.".into()),
            cover_url: Some("https://example.com/wd.jpg".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.title, "Walking Dead (Refreshed)");
    assert_eq!(
        updated.cv_id,
        Some(12345),
        "cv_id must not change on update"
    );
    assert_eq!(updated.publisher.as_deref(), Some("Skybound"));
    assert_eq!(updated.description.as_deref(), Some("Apocalypse comic."));
}

#[tokio::test]
async fn update_missing_id_returns_not_found() {
    let pool = fresh_pool().await;
    let err = series_repo::update(
        &pool,
        999,
        SeriesUpdate {
            title: "X".into(),
            sort_title: "x".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DbError::NotFound), "got {err:?}");
}

#[tokio::test]
async fn duplicate_cv_id_surfaces_unique_violation() {
    let pool = fresh_pool().await;
    series_repo::insert(&pool, walking_dead()).await.unwrap();
    let err = series_repo::insert(&pool, walking_dead())
        .await
        .unwrap_err();
    assert!(
        matches!(err, DbError::UniqueViolation { field: "cv_id" }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn null_cv_ids_are_independent_under_unique() {
    // SQLite treats multiple NULLs as distinct in UNIQUE columns. Two series
    // with `cv_id = NULL` should coexist.
    let pool = fresh_pool().await;
    let a = NewSeries {
        cv_id: None,
        metron_id: None,
        title: "A".into(),
        sort_title: "a".into(),
        start_year: None,
        publisher: None,
        description: None,
        cover_url: None,
    };
    let b = NewSeries {
        title: "B".into(),
        sort_title: "b".into(),
        ..a.clone()
    };
    series_repo::insert(&pool, a).await.unwrap();
    series_repo::insert(&pool, b).await.unwrap();
    let rows = series_repo::find_all(&pool).await.unwrap();
    assert_eq!(rows.len(), 2);
}

// ----- find_all_with_counts: solicited vs genuinely missing -----

async fn seeded_issue(
    pool: &sqlx::SqlitePool,
    series_id: i64,
    cv_issue_id: i64,
    number: &str,
    cover_date: Option<&str>,
) -> i64 {
    issue_repo::insert(
        pool,
        NewIssue {
            series_id,
            cv_issue_id: Some(cv_issue_id),
            metron_issue_id: None,
            number: number.into(),
            title: None,
            cover_date: cover_date.map(|s| s.into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

async fn mark_owned(pool: &sqlx::SqlitePool, library_root_id: i64, issue_id: i64, path: &str) {
    file_repo::insert(
        pool,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id,
            path_relative: path.into(),
            size_bytes: 1024,
            mtime: fixed_ts(),
            last_scanned_at: fixed_ts(),
            match_method: "phase_b".into(),
            match_confidence: 1.0,
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
}

#[tokio::test]
async fn find_all_with_counts_splits_solicited_from_missing() {
    // Setup: a series with 4 issues —
    //   #1 owned, past cover_date         → not missing, not solicited
    //   #2 missing, past cover_date       → missing, not solicited
    //   #3 missing, future cover_date     → missing AND solicited
    //   #4 missing, NULL cover_date       → missing, not solicited (guard)
    //
    // The UI uses `available = total - solicited = 3`, so this series
    // reads 1/3 (orange, genuinely incomplete) rather than 1/4. If a
    // user then matched #2, they would see 2/3 and #3 would still show
    // as solicited.
    let pool = fresh_pool().await;
    let library_root_id = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap()
    .id;
    let series_id = series_repo::insert(&pool, walking_dead()).await.unwrap().id;

    let i1 = seeded_issue(&pool, series_id, 101, "1", Some("2020-01-01")).await;
    let _i2 = seeded_issue(&pool, series_id, 102, "2", Some("2020-02-01")).await;
    let _i3 = seeded_issue(&pool, series_id, 103, "3", Some("2099-12-01")).await;
    let _i4 = seeded_issue(&pool, series_id, 104, "4", None).await;
    mark_owned(&pool, library_root_id, i1, "TWD 1.cbz").await;

    let counts = series_repo::find_all_with_counts(&pool).await.unwrap();
    let row = counts.iter().find(|r| r.series.id == series_id).unwrap();
    assert_eq!(row.total_count, 4);
    assert_eq!(row.owned_count, 1);
    assert_eq!(row.missing_count, 3);
    assert_eq!(
        row.solicited_count, 1,
        "only the 2099 issue should count as solicited"
    );
}

#[tokio::test]
async fn find_all_with_counts_owned_solicited_does_not_inflate() {
    // Edge case: a future-dated issue the user already owns (early
    // digital drop, advance reader copy) must NOT count as solicited.
    // The NOT EXISTS-owned guard is what enforces this.
    let pool = fresh_pool().await;
    let library_root_id = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap()
    .id;
    let series_id = series_repo::insert(&pool, walking_dead()).await.unwrap().id;

    let future_owned = seeded_issue(&pool, series_id, 201, "1", Some("2099-12-01")).await;
    mark_owned(&pool, library_root_id, future_owned, "TWD-future.cbz").await;

    let counts = series_repo::find_all_with_counts(&pool).await.unwrap();
    let row = counts.iter().find(|r| r.series.id == series_id).unwrap();
    assert_eq!(row.owned_count, 1);
    assert_eq!(row.missing_count, 0);
    assert_eq!(row.solicited_count, 0);
}
