//! Library Tidy Step 1 — repo tests for `discovered_folders_repo` and
//! the `series_repo` phantom surface.

mod common;

use common::{fixed_ts, fresh_pool};
use longbox_db::{
    discovered_folders_repo, file_repo, issue_repo, library_root_repo, pull_attempt_repo,
    pull_list_repo, series_repo, DbError, DiscoveredFolder, NewFile, NewIssue, NewLibraryRoot,
    NewPullAttempt, NewPullEntry, NewSeries,
};
use sqlx::SqlitePool;
use time::{Duration, PrimitiveDateTime};

// -------- discovered_folders_repo --------

fn folder(name: &str, file_count: i64) -> DiscoveredFolder {
    DiscoveredFolder {
        folder_name: name.into(),
        file_count,
    }
}

#[tokio::test]
async fn discovered_folder_upsert_inserts_new() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Wolverine (1982)", 24))
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].folder_name, "Wolverine (1982)");
    assert_eq!(rows[0].file_count, 24);
    assert!(rows[0].dismissed_at.is_none());
}

#[tokio::test]
async fn discovered_folder_upsert_refreshes_file_count() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Saga (2012)", 1))
        .await
        .unwrap();
    discovered_folders_repo::upsert(&pool, folder("Saga (2012)", 9))
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1, "re-detection updates, never duplicates");
    assert_eq!(rows[0].file_count, 9);
}

#[tokio::test]
async fn discovered_folder_upsert_skips_a_dismissed_folder() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Bone (1991)", 3))
        .await
        .unwrap();
    discovered_folders_repo::dismiss(&pool, &["Bone (1991)".into()])
        .await
        .unwrap();
    // Re-detected on a later scan — a dismissed folder must not resurface.
    discovered_folders_repo::upsert(&pool, folder("Bone (1991)", 5))
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert!(rows.is_empty(), "a dismissed folder stays dismissed");
}

#[tokio::test]
async fn discovered_folder_list_excludes_dismissed() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("A (2000)", 1))
        .await
        .unwrap();
    discovered_folders_repo::upsert(&pool, folder("B (2001)", 2))
        .await
        .unwrap();
    discovered_folders_repo::dismiss(&pool, &["A (2000)".into()])
        .await
        .unwrap();
    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].folder_name, "B (2001)");
}

#[tokio::test]
async fn discovered_folder_dismiss_is_idempotent_and_tolerates_unknowns() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("X (1999)", 1))
        .await
        .unwrap();
    let first = discovered_folders_repo::dismiss(&pool, &["X (1999)".into()])
        .await
        .unwrap();
    assert_eq!(first, 1);
    // Re-dismissing the same folder, plus an unknown one — both no-ops.
    let second = discovered_folders_repo::dismiss(&pool, &["X (1999)".into(), "never-seen".into()])
        .await
        .unwrap();
    assert_eq!(second, 0);
}

// -------- series_repo phantoms --------

async fn insert_series(pool: &SqlitePool, title: &str) -> i64 {
    series_repo::insert(
        pool,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
            start_year: Some(2010),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn update_last_matched_count_sets_the_value() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Saga").await;
    series_repo::update_last_matched_count(&pool, sid, 7)
        .await
        .unwrap();
    // No files -> the series is a phantom; its last_matched_count is now 7.
    let phantoms = series_repo::list_phantoms(&pool).await.unwrap();
    let row = phantoms.iter().find(|p| p.id == sid).expect("phantom row");
    assert_eq!(row.last_matched_count, 7);
}

#[tokio::test]
async fn update_last_matched_count_unknown_series_is_not_found() {
    let pool = fresh_pool().await;
    let err = series_repo::update_last_matched_count(&pool, 9999, 1)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound));
}

#[tokio::test]
async fn list_phantoms_excludes_series_with_an_owned_file() {
    let pool = fresh_pool().await;
    let phantom = insert_series(&pool, "Phantom Series").await;
    let owned = insert_series(&pool, "Owned Series").await;

    // Give `owned` an owned, present file.
    let library_root_id = library_root_repo::insert(
        &pool,
        NewLibraryRoot {
            path: "/comics".into(),
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        &pool,
        NewIssue {
            series_id: owned,
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
    file_repo::insert(
        &pool,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id,
            path_relative: "Owned Series/1.cbz".into(),
            size_bytes: 1,
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

    let ids: Vec<i64> = series_repo::list_phantoms(&pool)
        .await
        .unwrap()
        .iter()
        .map(|p| p.id)
        .collect();
    assert!(ids.contains(&phantom), "a zero-owned series is a phantom");
    assert!(
        !ids.contains(&owned),
        "a series with an owned, present file is not a phantom"
    );
}

#[tokio::test]
async fn list_phantoms_excludes_a_series_with_only_needs_review_files() {
    // Bug fix: auto-tidy was scheduling series for removal when every
    // file sat at sub-threshold confidence. needs_review files are
    // on-disk bytes — the series is not phantom.
    let pool = fresh_pool().await;
    let with_review = insert_series(&pool, "Below Threshold").await;
    give_file_with_status(&pool, with_review, "needs_review", 0.83).await;

    let ids: Vec<i64> = series_repo::list_phantoms(&pool)
        .await
        .unwrap()
        .iter()
        .map(|p| p.id)
        .collect();
    assert!(
        !ids.contains(&with_review),
        "a series whose only files are needs_review is not a phantom"
    );
}

#[tokio::test]
async fn list_phantoms_includes_a_series_whose_only_file_is_missing() {
    // A `needs_review` file that has gone missing from disk
    // (`is_present = 0`) does NOT keep its series alive — the new
    // predicate is about bytes on disk, not catalog state.
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Gone").await;
    give_file_with_status_and_presence(&pool, sid, "needs_review", 0.83, false).await;

    let ids: Vec<i64> = series_repo::list_phantoms(&pool)
        .await
        .unwrap()
        .iter()
        .map(|p| p.id)
        .collect();
    assert!(
        ids.contains(&sid),
        "a series whose only file is marked missing is still a phantom"
    );
}

// -------- series_repo auto-tidy (A.9 Step 6b) --------

/// Attach one owned, present file to `series_id` (via a fresh library
/// root + issue) so the series stops being a phantom.
async fn give_owned_file(pool: &SqlitePool, series_id: i64) {
    let library_root_id = library_root_repo::insert(
        pool,
        NewLibraryRoot {
            path: format!("/comics/{series_id}"),
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        pool,
        NewIssue {
            series_id,
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
    file_repo::insert(
        pool,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id,
            path_relative: format!("s{series_id}/1.cbz"),
            size_bytes: 1,
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
}

/// Attach one file with the given status + confidence + presence to
/// `series_id`. Powers the needs_review / unmatched / missing variants
/// of the "is this series a phantom" tests.
async fn give_file_with_status_and_presence(
    pool: &SqlitePool,
    series_id: i64,
    status: &str,
    confidence: f64,
    is_present: bool,
) {
    let library_root_id = library_root_repo::insert(
        pool,
        NewLibraryRoot {
            path: format!("/comics/{series_id}-{status}"),
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        pool,
        NewIssue {
            series_id,
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
    file_repo::insert(
        pool,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id,
            path_relative: format!("s{series_id}/1.cbz"),
            size_bytes: 1,
            mtime: fixed_ts(),
            last_scanned_at: fixed_ts(),
            match_method: "filename".into(),
            match_confidence: confidence,
            status: status.into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present,
            last_seen_at: fixed_ts(),
            matched_at: None,
        },
    )
    .await
    .unwrap();
}

async fn give_file_with_status(pool: &SqlitePool, series_id: i64, status: &str, confidence: f64) {
    give_file_with_status_and_presence(pool, series_id, status, confidence, true).await;
}

async fn empty_scan_counter(pool: &SqlitePool, series_id: i64) -> i64 {
    sqlx::query_scalar("SELECT consecutive_empty_scans FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn due_at(pool: &SqlitePool, series_id: i64) -> Option<PrimitiveDateTime> {
    sqlx::query_scalar("SELECT auto_tidy_due_at FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn auto_tidy_marks_a_series_empty_past_the_threshold() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Abandoned").await;
    let due = fixed_ts();

    // Two empty scans — below the threshold of 3, nothing is marked.
    series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    assert_eq!(empty_scan_counter(&pool, sid).await, 2);
    assert_eq!(
        series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap(),
        0,
        "two empty scans is below the threshold"
    );

    // The third tick reaches the threshold; the mark now fires.
    series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    assert_eq!(
        series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap(),
        1
    );
    assert_eq!(due_at(&pool, sid).await, Some(due));

    // A second mark pass is a no-op — the series is already marked.
    assert_eq!(
        series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap(),
        0,
        "an already-marked series is not re-marked"
    );
}

#[tokio::test]
async fn auto_tidy_never_marks_a_series_with_only_needs_review_files() {
    // Bug fix: the sweep was scheduling series for removal because
    // every file sat below the match-confidence threshold. The new
    // emptiness predicate counts needs_review / unmatched as keeping
    // the series alive — tick must not fire, mark must not fire.
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Sub-threshold").await;
    give_file_with_status(&pool, sid, "needs_review", 0.83).await;
    let due = fixed_ts();

    // Tick five times — well past the threshold of 3. The counter must
    // stay at 0 because a present file in any catalog-linked status
    // keeps the series alive.
    for _ in 0..5 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    assert_eq!(
        empty_scan_counter(&pool, sid).await,
        0,
        "a series with needs_review files must not tick its empty counter"
    );

    // And the sweep must not mark it even if the counter were somehow
    // elevated — the EXISTS guard in mark_for_auto_tidy catches it too.
    assert_eq!(
        series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap(),
        0
    );
    assert_eq!(due_at(&pool, sid).await, None);
}

#[tokio::test]
async fn auto_tidy_tick_clears_the_mark_when_needs_review_files_appear() {
    // A series marked for tidy regains a needs_review file (e.g. user
    // dropped CBZs back into the folder but they haven't matched
    // anything yet). The next tick must clear the mark — the
    // auto-recovery path includes any present file, not just owned.
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Came Back Sub-threshold").await;
    let due = fixed_ts();

    for _ in 0..3 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap();
    assert_eq!(due_at(&pool, sid).await, Some(due));

    give_file_with_status(&pool, sid, "needs_review", 0.50).await;
    series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    assert_eq!(empty_scan_counter(&pool, sid).await, 0);
    assert_eq!(due_at(&pool, sid).await, None);
}

#[tokio::test]
async fn auto_tidy_never_marks_a_series_awaiting_a_first_download() {
    let pool = fresh_pool().await;
    let on_list = insert_series(&pool, "On Pull List").await;
    let attempted = insert_series(&pool, "Has Pull Attempt").await;
    let due = fixed_ts();

    pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id: on_list,
            start_issue: None,
        },
    )
    .await
    .unwrap();

    // `attempted` has a pull attempt but no pull-list row.
    let issue_id = issue_repo::insert(
        &pool,
        NewIssue {
            series_id: attempted,
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
    pull_attempt_repo::insert(
        &pool,
        NewPullAttempt {
            series_id: attempted,
            issue_id,
            indexer_id: None,
            release_id: None,
            status: "pending".into(),
            error_message: None,
            retry_count: 0,
            download_handle: None,
        },
    )
    .await
    .unwrap();

    // Both are empty well past the threshold...
    for _ in 0..5 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    // ...but neither is marked: a pull-list entry or a pull attempt is
    // an explicit "awaiting a first download" intent auto-tidy must not
    // act on.
    assert_eq!(
        series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap(),
        0
    );
    assert_eq!(due_at(&pool, on_list).await, None);
    assert_eq!(due_at(&pool, attempted).await, None);
}

#[tokio::test]
async fn auto_tidy_purge_respects_the_recovery_window() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Doomed").await;
    let due = fixed_ts();

    for _ in 0..3 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap();

    // A purge before the deadline leaves the series alone.
    assert_eq!(
        series_repo::purge_due_auto_tidy(&pool, due - Duration::days(1))
            .await
            .unwrap(),
        0
    );
    assert!(series_repo::find_by_id(&pool, sid).await.unwrap().is_some());

    // A purge once the deadline has passed hard-deletes it.
    assert_eq!(
        series_repo::purge_due_auto_tidy(&pool, due + Duration::days(1))
            .await
            .unwrap(),
        1
    );
    assert!(series_repo::find_by_id(&pool, sid).await.unwrap().is_none());
}

#[tokio::test]
async fn auto_tidy_tick_clears_the_mark_when_files_return() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Came Back").await;
    let due = fixed_ts();

    for _ in 0..3 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap();
    assert_eq!(due_at(&pool, sid).await, Some(due));

    // The folder reappears: the series owns a present file again. The
    // next tick resets the counter and clears the removal mark.
    give_owned_file(&pool, sid).await;
    series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    assert_eq!(empty_scan_counter(&pool, sid).await, 0);
    assert_eq!(due_at(&pool, sid).await, None);
}

#[tokio::test]
async fn keep_phantom_series_clears_every_auto_tidy_signal() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Kept").await;
    let due = fixed_ts();

    series_repo::update_last_matched_count(&pool, sid, 9)
        .await
        .unwrap();
    for _ in 0..3 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap();

    series_repo::keep_phantom_series(&pool, sid).await.unwrap();

    assert_eq!(empty_scan_counter(&pool, sid).await, 0);
    assert_eq!(due_at(&pool, sid).await, None);

    // The whole point of the fix: a kept series leaves the phantom list
    // entirely (durable `tidy_exempt = 1`) and surfaces in the kept list.
    assert!(
        !series_repo::list_phantoms(&pool)
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == sid),
        "a kept series must not appear in list_phantoms"
    );
    let kept = series_repo::list_kept_series(&pool)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == sid)
        .expect("kept series shows in list_kept_series");
    assert_eq!(kept.last_matched_count, 0);
    assert!(kept.auto_tidy_due_at.is_none());
}

#[tokio::test]
async fn kept_series_does_not_tick_empty_scan_counter() {
    // Root cause of the original bug: keeping reset the counter, then the
    // next scan ticked it right back up and the series re-marked. With
    // the exemption flag the tick must skip the series entirely — the
    // counter stays at 0 no matter how many empty scans run.
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Exempt").await;

    series_repo::keep_phantom_series(&pool, sid).await.unwrap();
    for _ in 0..5 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    assert_eq!(
        empty_scan_counter(&pool, sid).await,
        0,
        "a tidy-exempt series must never tick its empty-scan counter"
    );
}

#[tokio::test]
async fn kept_series_is_not_marked_for_auto_tidy() {
    // Even if the counter were somehow past the threshold, the exemption
    // guard in mark_for_auto_tidy must refuse to schedule a kept series.
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Exempt").await;
    let due = fixed_ts();

    // Accrue empty scans first, then keep — proves the guard, not just a
    // zeroed counter, is what stops the mark.
    for _ in 0..5 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    series_repo::keep_phantom_series(&pool, sid).await.unwrap();
    // Force the counter back above threshold to defeat the zeroing path.
    sqlx::query("UPDATE series SET consecutive_empty_scans = 9 WHERE id = ?")
        .bind(sid)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap(),
        0,
        "a tidy-exempt series must never be marked for removal"
    );
    assert_eq!(due_at(&pool, sid).await, None);
}

#[tokio::test]
async fn unkeep_phantom_series_restores_normal_behavior() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Reconsidered").await;
    let due = fixed_ts();

    series_repo::keep_phantom_series(&pool, sid).await.unwrap();
    series_repo::unkeep_phantom_series(&pool, sid).await.unwrap();

    // Back in the phantom list, gone from the kept list.
    assert!(
        series_repo::list_phantoms(&pool)
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == sid),
        "an un-kept series returns to the phantom list"
    );
    assert!(
        !series_repo::list_kept_series(&pool)
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == sid),
    );

    // And the scanner re-evaluates it normally: ticks resume, and once
    // past the threshold it can be marked again.
    for _ in 0..3 {
        series_repo::tick_empty_scan_counters(&pool).await.unwrap();
    }
    assert_eq!(empty_scan_counter(&pool, sid).await, 3);
    assert_eq!(
        series_repo::mark_for_auto_tidy(&pool, 3, due).await.unwrap(),
        1,
        "an un-kept series is schedulable again after fresh empty scans"
    );
}

#[tokio::test]
async fn unkeep_phantom_series_unknown_is_not_found() {
    let pool = fresh_pool().await;
    let err = series_repo::unkeep_phantom_series(&pool, 4242)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound));
}

#[tokio::test]
async fn keep_phantom_series_unknown_is_not_found() {
    let pool = fresh_pool().await;
    let err = series_repo::keep_phantom_series(&pool, 4242)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound));
}

// -------- A.9 hot-fix: bulk-convert dedup --------

#[tokio::test]
async fn find_for_dedup_matches_on_exact_sort_title_and_year() {
    let pool = fresh_pool().await;
    let saga = insert_series(&pool, "Saga").await;
    insert_series(&pool, "Other").await;

    let hit = series_repo::find_for_dedup(&pool, "saga", Some(2010))
        .await
        .unwrap();
    assert_eq!(hit.map(|r| r.id), Some(saga));

    // A different year is a different series.
    let miss = series_repo::find_for_dedup(&pool, "saga", Some(2011))
        .await
        .unwrap();
    assert!(miss.is_none());
}

#[tokio::test]
async fn find_for_dedup_treats_null_start_year_as_a_value_not_a_wildcard() {
    let pool = fresh_pool().await;
    // Pattern C: a folder lacking `(YYYY)` parses to start_year=None.
    // SQLite's `=` returns NULL on NULL operands; the repo uses `IS`
    // so NULL-year rows dedup against each other but not against
    // non-NULL queries.
    let sid = series_repo::insert(
        &pool,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Last Barbarians".into(),
            sort_title: "last barbarians".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    let hit = series_repo::find_for_dedup(&pool, "last barbarians", None)
        .await
        .unwrap();
    assert_eq!(hit.map(|r| r.id), Some(sid));

    // A non-NULL query does not collapse with the NULL-year row.
    // (A.9 Bug 2 asymmetric design: phase-2 fallback fires only for
    // NULL incoming, not for year-set incoming.)
    let with_year = series_repo::find_for_dedup(&pool, "last barbarians", Some(2010))
        .await
        .unwrap();
    assert!(with_year.is_none());
}

#[tokio::test]
async fn find_for_dedup_null_incoming_falls_back_to_year_set_row() {
    // A.9 Bug 2: when phase 1 returns nothing AND incoming year is
    // NULL, phase 2 falls back to a year-set row sharing the
    // sort_title. The observed shape: user has the same series under
    // two folder names (`Enfield Gang Massacre (2024)` and
    // `The Enfield Gang Massacre`) — second convert links to first
    // instead of creating a dupe.
    let pool = fresh_pool().await;
    let with_year = series_repo::insert(
        &pool,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Enfield Gang Massacre".into(),
            sort_title: "enfield gang massacre".into(),
            start_year: Some(2024),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    let hit = series_repo::find_for_dedup(&pool, "enfield gang massacre", None)
        .await
        .unwrap();
    assert_eq!(
        hit.map(|r| r.id),
        Some(with_year),
        "NULL-year incoming links to the year-set row"
    );
}

#[tokio::test]
async fn find_for_dedup_null_incoming_does_not_match_when_multiple_year_set_rows() {
    // A.9 Bug 2 multi-match guard: if more than one year-set row
    // shares the sort_title (e.g. 1964 + 2024 Daredevil both
    // catalogued), phase 2 refuses the link. Ambiguous case returns
    // None; the convert creates a fresh shallow row and the user
    // resolves the merge manually.
    let pool = fresh_pool().await;
    series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(1001),
            metron_id: None,
            title: "Daredevil".into(),
            sort_title: "daredevil".into(),
            start_year: Some(1964),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(2024),
            metron_id: None,
            title: "Daredevil".into(),
            sort_title: "daredevil".into(),
            start_year: Some(2024),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    let result = series_repo::find_for_dedup(&pool, "daredevil", None)
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "ambiguous match (multiple year-set rows) refuses to link"
    );
}

#[tokio::test]
async fn find_for_dedup_prefers_a_cv_id_set_row_over_a_shallow_one() {
    // In steady state the new idempotency prevents duplicates, but
    // pre-cleanup dupes can briefly co-exist. The survivor rule:
    // cv_id-set first, then earliest created_at.
    let pool = fresh_pool().await;
    insert_series(&pool, "Same Title").await; // cv_id NULL (shallow)
    let with_cv = series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(99),
            metron_id: None,
            title: "Same Title".into(),
            sort_title: "same title".into(),
            start_year: Some(2010),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    let hit = series_repo::find_for_dedup(&pool, "same title", Some(2010))
        .await
        .unwrap();
    assert_eq!(
        hit.map(|r| r.id),
        Some(with_cv),
        "the cv_id-set row outranks the shallow one"
    );
}

#[tokio::test]
async fn upsert_number_only_returning_creates_missing_and_preserves_existing() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Series").await;
    // A pre-existing CV-canonical issue at number "2".
    let existing = issue_repo::insert(
        &pool,
        NewIssue {
            series_id: sid,
            cv_issue_id: Some(123),
            metron_issue_id: None,
            number: "2".into(),
            title: Some("CV Title".into()),
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // Upsert "1", "2", "3" — "2" already exists, "1" and "3" are
    // synthesized fresh.
    let resolved = issue_repo::upsert_number_only_returning(
        &pool,
        sid,
        &["1".into(), "2".into(), "3".into()],
    )
    .await
    .unwrap();
    let by_number: std::collections::HashMap<String, i64> = resolved.into_iter().collect::<Vec<_>>()
        .into_iter()
        .map(|(id, n)| (n, id))
        .collect();
    assert_eq!(by_number.len(), 3);
    assert_eq!(
        by_number["2"], existing.id,
        "the pre-existing issue's id is returned unchanged"
    );

    // The pre-existing CV issue's CV id and title are preserved — the
    // `DO UPDATE SET number = excluded.number` is a no-op assignment
    // that exists only to make RETURNING fire on conflict.
    let still = issue_repo::find_by_id(&pool, existing.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.cv_issue_id, Some(123));
    assert_eq!(still.title.as_deref(), Some("CV Title"));
}

#[tokio::test]
async fn upsert_number_only_returning_empty_input_is_a_no_op() {
    let pool = fresh_pool().await;
    let sid = insert_series(&pool, "Empty").await;
    let resolved = issue_repo::upsert_number_only_returning(&pool, sid, &[])
        .await
        .unwrap();
    assert!(resolved.is_empty());
}

// -------- A.9 hot-fix item F6: discovered_folders auto-dismiss --------

#[tokio::test]
async fn discovered_folders_auto_dismiss_not_in_clears_stale_open_rows() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("A (2010)", 1))
        .await
        .unwrap();
    discovered_folders_repo::upsert(&pool, folder("B (2011)", 1))
        .await
        .unwrap();
    discovered_folders_repo::upsert(&pool, folder("C (2012)", 1))
        .await
        .unwrap();

    // Only A and C are still untracked this scan; B is stale.
    let keep: std::collections::HashSet<String> =
        ["A (2010)".into(), "C (2012)".into()].into_iter().collect();
    let dismissed = discovered_folders_repo::auto_dismiss_not_in(&pool, &keep)
        .await
        .unwrap();
    assert_eq!(dismissed, 1, "exactly the one stale folder is auto-dismissed");

    let names: Vec<String> = discovered_folders_repo::list(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.folder_name)
        .collect();
    assert!(names.contains(&"A (2010)".to_string()));
    assert!(names.contains(&"C (2012)".to_string()));
    assert!(!names.contains(&"B (2011)".to_string()));
}

#[tokio::test]
async fn discovered_folders_auto_dismiss_not_in_empty_keep_dismisses_every_open_row() {
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Whatever", 1))
        .await
        .unwrap();

    let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
    let dismissed = discovered_folders_repo::auto_dismiss_not_in(&pool, &empty)
        .await
        .unwrap();
    assert_eq!(dismissed, 1);
    assert!(discovered_folders_repo::list(&pool).await.unwrap().is_empty());
}

// -------- A.9 F6 hot-fix: split-source dismiss --------

#[tokio::test]
async fn auto_dismissed_folder_resurfaces_when_re_detected() {
    // The F6 trap fix: after auto_dismiss writes auto_dismissed_at,
    // the next upsert (re-detection) clears it and the row reappears
    // in `list()`. Pre-fix the WHERE dismissed_at IS NULL guard
    // silently swallowed re-detection.
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Resurrect Me (2020)", 5))
        .await
        .unwrap();
    discovered_folders_repo::auto_dismiss(&pool, &["Resurrect Me (2020)".into()])
        .await
        .unwrap();
    assert!(
        discovered_folders_repo::list(&pool).await.unwrap().is_empty(),
        "auto-dismissed row is hidden from the list"
    );

    // Re-detection: files in this folder became unmatched again.
    discovered_folders_repo::upsert(&pool, folder("Resurrect Me (2020)", 7))
        .await
        .unwrap();

    let rows = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1, "row resurfaces after re-detection");
    assert_eq!(rows[0].folder_name, "Resurrect Me (2020)");
    assert_eq!(rows[0].file_count, 7, "refreshed file_count on resurface");
    assert!(rows[0].auto_dismissed_at.is_none(), "auto_dismissed_at cleared");
    assert!(rows[0].dismissed_at.is_none(), "user dismissed_at stays null");
}

#[tokio::test]
async fn user_dismissed_folder_stays_hidden_when_re_detected() {
    // User-permanent dismiss: even re-detection cannot resurface a
    // folder the user explicitly dismissed. The upsert's
    // `WHERE dismissed_at IS NULL` guard skips the conflict update
    // entirely, so the row stays as the user left it.
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Never Show (2021)", 3))
        .await
        .unwrap();
    discovered_folders_repo::dismiss(&pool, &["Never Show (2021)".into()])
        .await
        .unwrap();
    assert!(discovered_folders_repo::list(&pool).await.unwrap().is_empty());

    // Re-detection attempt — should be a no-op on the user-dismissed row.
    discovered_folders_repo::upsert(&pool, folder("Never Show (2021)", 99))
        .await
        .unwrap();

    assert!(
        discovered_folders_repo::list(&pool).await.unwrap().is_empty(),
        "user-dismissed folder stays hidden even after re-detection"
    );
}

#[tokio::test]
async fn combined_user_and_auto_dismiss_stays_hidden_after_auto_clear() {
    // A folder both user-dismissed and (later) auto-dismissed. An
    // upsert can clear the auto column, but the user column is
    // preserved by the WHERE guard — the row stays hidden.
    let pool = fresh_pool().await;
    discovered_folders_repo::upsert(&pool, folder("Both Dismissed (2022)", 2))
        .await
        .unwrap();
    discovered_folders_repo::auto_dismiss(&pool, &["Both Dismissed (2022)".into()])
        .await
        .unwrap();
    discovered_folders_repo::dismiss(&pool, &["Both Dismissed (2022)".into()])
        .await
        .unwrap();

    discovered_folders_repo::upsert(&pool, folder("Both Dismissed (2022)", 5))
        .await
        .unwrap();

    // The upsert is a no-op against a user-dismissed row, so even the
    // auto column stays set (the WHERE clause guarded the SET).
    assert!(
        discovered_folders_repo::list(&pool).await.unwrap().is_empty(),
        "user-dismiss still hides the row regardless of auto state"
    );
}
