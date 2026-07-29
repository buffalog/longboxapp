mod common;

use common::{fixed_ts, fresh_pool};
use longbox_db::{
    file_repo, issue_repo, library_root_repo, series_repo, DbError, FileUpdate, NewFile, NewIssue,
    NewLibraryRoot, NewSeries,
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
        matched_at: if issue_id.is_some() {
            Some(fixed_ts())
        } else {
            None
        },
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
    file_repo::insert(&pool, new_file(library_root_id, "b.cbz", "unmatched", None))
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
    file_repo::insert(&pool, new_file(library_root_id, "b.cbz", "unmatched", None))
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
            matched_at: None,
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
    let row = file_repo::insert(&pool, new_file(library_root_id, "a.cbz", "unmatched", None))
        .await
        .unwrap();
    file_repo::delete(&pool, row.id).await.unwrap();
    assert!(file_repo::find_by_id(&pool, row.id)
        .await
        .unwrap()
        .is_none());
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
    let row = file_repo::insert(&pool, new_file(library_root_id, "a.cbz", "unmatched", None))
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
    assert!(
        other_row.is_present,
        "other library root's row must remain untouched"
    );
}

// ----- upsert_imported (Phase B step 3) -----

fn now_offset() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
}

#[tokio::test]
async fn upsert_imported_fresh_row_sets_phase_b_owned() {
    let pool = fresh_pool().await;
    let (library_root_id, series_id, issue_id) = seed(&pool).await;
    let row = file_repo::upsert_imported(
        &pool,
        library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
        series_id,
        issue_id,
        "phase_b",
        17_823_412,
        now_offset(),
    )
    .await
    .unwrap();
    assert_eq!(row.issue_id, Some(issue_id));
    assert_eq!(row.status, "owned");
    assert_eq!(row.match_method, "phase_b");
    assert!((row.match_confidence - 1.0).abs() < f64::EPSILON);
    assert!(row.is_present);
    assert!(
        row.matched_at.is_some(),
        "matched_at must be set on first import"
    );
}

#[tokio::test]
async fn upsert_imported_same_issue_preserves_matched_at() {
    let pool = fresh_pool().await;
    let (library_root_id, series_id, issue_id) = seed(&pool).await;
    let first = file_repo::upsert_imported(
        &pool,
        library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
        series_id,
        issue_id,
        "phase_b",
        1000,
        now_offset(),
    )
    .await
    .unwrap();

    // Sleep enough that "now" measurably differs at second-of-day
    // granularity (the column stores PrimitiveDateTime; second precision
    // is enough to detect a change if the rule went wrong).
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let second = file_repo::upsert_imported(
        &pool,
        library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
        series_id,
        issue_id, // same issue
        "phase_b",
        1000,
        now_offset(),
    )
    .await
    .unwrap();

    assert_eq!(
        second.id, first.id,
        "should be an UPDATE, not a fresh INSERT"
    );
    assert_eq!(
        second.matched_at, first.matched_at,
        "same issue_id must preserve matched_at (per Task 3 rule)"
    );
    assert_ne!(
        second.last_seen_at, first.last_seen_at,
        "last_seen_at should bump on every upsert"
    );
}

#[tokio::test]
async fn upsert_imported_changed_issue_bumps_matched_at() {
    let pool = fresh_pool().await;
    let (library_root_id, series_id, first_issue) = seed(&pool).await;
    let second_issue = issue_repo::insert(
        &pool,
        NewIssue {
            series_id,
            cv_issue_id: Some(102),
            metron_issue_id: None,
            number: "2".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    let first = file_repo::upsert_imported(
        &pool,
        library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
        series_id,
        first_issue,
        "phase_b",
        1000,
        now_offset(),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let second = file_repo::upsert_imported(
        &pool,
        library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
        series_id,
        second_issue, // different issue
        "phase_b",
        1000,
        now_offset(),
    )
    .await
    .unwrap();

    assert_eq!(second.id, first.id);
    assert_eq!(second.issue_id, Some(second_issue));
    assert!(
        second.matched_at > first.matched_at,
        "different issue_id must bump matched_at to now"
    );
}

#[tokio::test]
async fn upsert_imported_promotes_lower_confidence_match_methods() {
    // Codifies "Phase B is the higher-confidence path." A row that was
    // matched via filename or ComicInfo earlier must be unconditionally
    // promoted to match_method='phase_b' when Phase B re-imports it.
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;

    for prior_method in ["filename_regex", "comicinfo_xml", "web_url_cv"] {
        let path = format!("seed-{prior_method}.cbz");
        file_repo::insert(
            &pool,
            NewFile {
                issue_id: Some(issue_id),
                library_root_id,
                path_relative: path.clone(),
                size_bytes: 999,
                mtime: fixed_ts(),
                last_scanned_at: fixed_ts(),
                match_method: prior_method.to_string(),
                match_confidence: 0.85,
                status: "owned".to_string(),
                cached_comicinfo_xml: None,
                cached_at: None,
                is_present: true,
                last_seen_at: fixed_ts(),
                matched_at: Some(fixed_ts()),
            },
        )
        .await
        .unwrap();

        let after = file_repo::upsert_imported(
            &pool,
            library_root_id,
            &path,
            1,
            issue_id,
            "phase_b",
            999,
            now_offset(),
        )
        .await
        .unwrap();
        assert_eq!(
            after.match_method, "phase_b",
            "prior method {prior_method:?} must be promoted to phase_b"
        );
        assert!(
            (after.match_confidence - 1.0).abs() < f64::EPSILON,
            "confidence must be 1.0 after phase_b promotion (was {})",
            after.match_confidence
        );
    }
}

#[tokio::test]
async fn upsert_imported_clears_stale_comicinfo_cache() {
    // Phase B writes a fresh ComicInfo into the file as part of its
    // pipeline; any prior cached XML in the DB is now stale.
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;
    file_repo::insert(
        &pool,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id,
            path_relative: "stale.cbz".into(),
            size_bytes: 100,
            mtime: fixed_ts(),
            last_scanned_at: fixed_ts(),
            match_method: "comicinfo_xml".into(),
            match_confidence: 0.9,
            status: "owned".into(),
            cached_comicinfo_xml: Some("<ComicInfo>stale data</ComicInfo>".into()),
            cached_at: Some(fixed_ts()),
            is_present: true,
            last_seen_at: fixed_ts(),
            matched_at: Some(fixed_ts()),
        },
    )
    .await
    .unwrap();

    let after = file_repo::upsert_imported(
        &pool,
        library_root_id,
        "stale.cbz",
        1,
        issue_id,
        "phase_b",
        100,
        now_offset(),
    )
    .await
    .unwrap();
    assert_eq!(after.cached_comicinfo_xml, None);
    assert_eq!(after.cached_at, None);
}

// ----- purge_absent_ghosts_for_issue (Phase B move cleanup) -----

async fn flip_absent(pool: &SqlitePool, id: i64) {
    sqlx::query!(r#"UPDATE files SET is_present = 0 WHERE id = ?"#, id)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn purge_absent_ghosts_drops_absent_row_at_other_path() {
    // The reproduction: scanner cataloged a file at `_unsorted/foo.cbr`
    // (is_present flipped to false on a later scan after Phase B moved
    // it), then Phase B writes the new row at the canonical location.
    // The purge must drop the stale `_unsorted` row.
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;

    let ghost = file_repo::insert(
        &pool,
        new_file(
            library_root_id,
            "_unsorted/saga 1.cbr",
            "owned",
            Some(issue_id),
        ),
    )
    .await
    .unwrap();
    flip_absent(&pool, ghost.id).await;

    let kept = "Saga (2012)/Saga (2012) 001.cbz";
    file_repo::upsert_imported(
        &pool,
        library_root_id,
        kept,
        _series_id,
        issue_id,
        "phase_b",
        17_823_412,
        now_offset(),
    )
    .await
    .unwrap();

    let purged = file_repo::purge_absent_ghosts_for_issue(&pool, library_root_id, issue_id, kept)
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(file_repo::find_by_id(&pool, ghost.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn purge_absent_ghosts_keeps_present_row_at_other_path() {
    // Load-bearing guard: a legitimate second copy of the same issue
    // (different format / edition still on disk) must NOT be purged.
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;

    let other_copy = file_repo::insert(
        &pool,
        new_file(
            library_root_id,
            "Saga (2012)/Saga (2012) 001 (alt).cbz",
            "owned",
            Some(issue_id),
        ),
    )
    .await
    .unwrap();
    // is_present stays true — the second copy is still on disk.

    let purged = file_repo::purge_absent_ghosts_for_issue(
        &pool,
        library_root_id,
        issue_id,
        "Saga (2012)/Saga (2012) 001.cbz",
    )
    .await
    .unwrap();
    assert_eq!(purged, 0);
    assert!(file_repo::find_by_id(&pool, other_copy.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn purge_absent_ghosts_keeps_kept_path_row() {
    // The row that was just upserted (matching `kept_path`) must never
    // be purged, even if for some reason it were also marked absent.
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;

    let kept_path = "Saga (2012)/Saga (2012) 001.cbz";
    let kept = file_repo::insert(
        &pool,
        new_file(library_root_id, kept_path, "owned", Some(issue_id)),
    )
    .await
    .unwrap();
    flip_absent(&pool, kept.id).await;

    let purged =
        file_repo::purge_absent_ghosts_for_issue(&pool, library_root_id, issue_id, kept_path)
            .await
            .unwrap();
    assert_eq!(purged, 0);
    assert!(file_repo::find_by_id(&pool, kept.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn purge_absent_ghosts_scoped_to_library_root() {
    // Another library root holding an absent row for the same issue
    // must be left alone — different library_root_id = different scope.
    let pool = fresh_pool().await;
    let (library_root_id, _series_id, issue_id) = seed(&pool).await;
    let other_root_id = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/other".into(),
        },
    )
    .await
    .unwrap()
    .id;

    let foreign = file_repo::insert(
        &pool,
        new_file(
            other_root_id,
            "_unsorted/saga 1.cbr",
            "owned",
            Some(issue_id),
        ),
    )
    .await
    .unwrap();
    flip_absent(&pool, foreign.id).await;

    let purged = file_repo::purge_absent_ghosts_for_issue(
        &pool,
        library_root_id, // local root
        issue_id,
        "Saga (2012)/Saga (2012) 001.cbz",
    )
    .await
    .unwrap();
    assert_eq!(purged, 0);
    assert!(file_repo::find_by_id(&pool, foreign.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn upsert_imported_with_invalid_issue_id_errors() {
    let pool = fresh_pool().await;
    let (library_root_id, series_id, _) = seed(&pool).await;
    let err = file_repo::upsert_imported(
        &pool,
        library_root_id,
        "nope.cbz",
        series_id,
        9999, // FK doesn't exist
        "phase_b",
        100,
        now_offset(),
    )
    .await
    .unwrap_err();
    // SQLite FK violations surface as Other (no special-casing in the
    // current error enum); the important property is that we don't
    // silently succeed.
    assert!(matches!(err, DbError::Other(_)), "got {err:?}");
}

/// The unlink must select by BINDING, not by path.
///
/// `files.issue_id` is the only record of the association and
/// `ON DELETE SET NULL` destroys it, so the unlink has to run BEFORE the
/// issues are deleted. The version this replaces ran after, and could
/// therefore only match on a folder-name prefix — which silently missed every
/// file of the series living anywhere else. On the live catalog that left 186
/// rows at `issue_id = NULL, status = 'owned'`: owned, pointing at nothing,
/// and skipped by `rematch_for_series`, which only selects `needs_review`.
/// 109 of those 186 sit outside any current series folder, so no prefix could
/// ever have reached them.
#[tokio::test]
async fn unlink_by_series_reaches_files_outside_the_series_folder() {
    let pool = fresh_pool().await;
    let (library_root_id, series_id, issue_id) = seed(&pool).await;

    // Inside the folder a prefix match would have found.
    let inside = file_repo::insert(
        &pool,
        new_file(
            library_root_id,
            "Saga (2012)/Saga (2012) 001.cbz",
            "owned",
            Some(issue_id),
        ),
    )
    .await
    .unwrap()
    .id;
    // Outside it — moved, re-foldered, or staged. Bound to the same issue,
    // and invisible to any prefix built from the series folder name.
    let outside = file_repo::insert(
        &pool,
        new_file(
            library_root_id,
            "_unsorted/Saga 001 (2012).cbz",
            "owned",
            Some(issue_id),
        ),
    )
    .await
    .unwrap()
    .id;
    // A different series' file must not be touched.
    let other_series = series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(2),
            metron_id: None,
            title: "Nailbiter".into(),
            sort_title: "nailbiter".into(),
            start_year: Some(2014),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let other_issue = issue_repo::insert(
        &pool,
        NewIssue {
            series_id: other_series,
            cv_issue_id: Some(201),
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
    let untouched = file_repo::insert(
        &pool,
        new_file(
            library_root_id,
            "Nailbiter (2014)/Nailbiter 001.cbz",
            "owned",
            Some(other_issue),
        ),
    )
    .await
    .unwrap()
    .id;

    let n = file_repo::unlink_by_series(&pool, series_id).await.unwrap();
    assert_eq!(n, 2, "both of the series' files, wherever they live");

    for id in [inside, outside] {
        let row = file_repo::find_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.issue_id, None, "file {id} must be unlinked");
        assert_eq!(
            row.status, "needs_review",
            "file {id} must land where rematch_for_series will actually select it"
        );
    }
    let row = file_repo::find_by_id(&pool, untouched).await.unwrap().unwrap();
    assert_eq!(row.issue_id, Some(other_issue), "other series untouched");
    assert_eq!(row.status, "owned");
}

/// The impossible state, asserted as an invariant: after unlinking, no row is
/// left `owned` with no issue. That combination is what `classify_status`
/// can never produce and no consumer can reach.
#[tokio::test]
async fn unlink_leaves_no_owned_row_without_an_issue() {
    let pool = fresh_pool().await;
    let (library_root_id, series_id, issue_id) = seed(&pool).await;
    for path in ["Saga (2012)/Saga 001.cbz", "elsewhere/Saga 002.cbz"] {
        file_repo::insert(&pool, new_file(library_root_id, path, "owned", Some(issue_id)))
            .await
            .unwrap();
    }

    file_repo::unlink_by_series(&pool, series_id).await.unwrap();
    // Now delete the issues, as the caller does immediately after.
    issue_repo::delete_by_series(&pool, series_id).await.unwrap();

    let trapped = file_repo::list_by_library_root(&pool, library_root_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|f| f.issue_id.is_none() && f.status == "owned")
        .count();
    assert_eq!(trapped, 0, "no row may be left owned and pointing at nothing");
}
