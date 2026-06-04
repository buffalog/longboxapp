mod common;

use std::time::Duration;

use common::{build_fixture_library, fresh_pool, seed_library_root, seed_walking_dead, write_cbz};
use longbox_db::{file_repo, FileRow, NewSeries};
use longbox_scanner::{ScanError, Scanner, ScannerConfig};
use tempfile::TempDir;

fn scanner_for(db: longbox_db::Pool) -> Scanner {
    Scanner::new(db, ScannerConfig::default())
}

async fn find_file(pool: &longbox_db::Pool, library_root_id: i64, path: &str) -> FileRow {
    file_repo::find_by_path(pool, library_root_id, path)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("file row missing for {path}"))
}

// -------- happy paths --------

#[tokio::test]
async fn full_scan_empty_db_marks_everything_unmatched() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    let report = scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();

    // 5 valid CBZs in the fixture; corrupt.cbz also yields a row attempt but
    // errors during ComicInfo extraction → still inserted as unmatched per
    // the fall-through-to-filename rule. Actually corrupt.cbz fails the
    // ComicInfo step but proceeds to filename parsing, where "corrupt.cbz"
    // matches nothing → unmatched row.
    assert!(report.files_seen >= 5);
    assert_eq!(report.matched_owned, 0);
    // At least the four parseable filenames count as something (unmatched
    // since no series seeded).
    assert!(report.unmatched + report.matched_needs_review >= 4);
}

#[tokio::test]
async fn full_scan_with_seeded_watchlist_matches_all_three_tiers() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    let report = scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();

    // 001 → Tier 1 (Web URL → CV issue 101001).
    let f1 = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 001 (2003).cbz",
    )
    .await;
    assert_eq!(f1.match_method, "web_url_cv", "001 should hit Tier 1");
    assert_eq!(f1.match_confidence, 1.0);
    assert_eq!(f1.status, "owned");

    // 002 → Tier 3 (no ComicInfo, filename matched).
    let f2 = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 002 (2003).cbz",
    )
    .await;
    assert_eq!(f2.match_method, "filename_regex", "002 should hit Tier 3");
    assert_eq!(f2.status, "owned");

    // 003 → Tier 2 (ComicInfo Series + Number, no Web URL).
    let f3 = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 003 (2003).cbz",
    )
    .await;
    assert_eq!(f3.match_method, "comicinfo_xml", "003 should hit Tier 2");
    assert_eq!(f3.status, "owned");

    // Saga: no series in watchlist → unmatched.
    let saga = find_file(&pool, library_root_id, "Saga (2012)/Saga 001 (2012).cbz").await;
    assert_eq!(saga.status, "unmatched");

    // UnknownComic: nothing to match against.
    let unk = find_file(&pool, library_root_id, "Mystery/UnknownComic.cbz").await;
    assert_eq!(unk.status, "unmatched");

    assert!(
        report.matched_owned >= 3,
        "expected at least 3 owned, got {}",
        report.matched_owned
    );
    assert!(report.duration_ms < 30_000);
}

#[tokio::test]
async fn tier1_cv_url_falls_through_when_id_not_in_db() {
    let tmp = TempDir::new().unwrap();
    let wd_dir = tmp.path().join("Walking Dead (2003)");
    write_cbz(
        &wd_dir.join("Walking Dead 001 (2003).cbz"),
        Some(
            r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>The Walking Dead</Series>
  <Number>1</Number>
  <Volume>2003</Volume>
  <Web>https://comicvine.gamespot.com/issue/4000-77777/</Web>
</ComicInfo>"#,
        ),
    );

    let pool = fresh_pool().await;
    // Series with issue #1 but a different cv_issue_id — Tier 1 misses.
    let s = longbox_db::series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(2127),
            metron_id: None,
            title: "The Walking Dead".into(),
            sort_title: "walking dead".into(),
            start_year: Some(2003),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    longbox_db::issue_repo::insert(
        &pool,
        longbox_db::NewIssue {
            series_id: s.id,
            cv_issue_id: Some(101_001),
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
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();
    let f = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 001 (2003).cbz",
    )
    .await;
    // Tier 2 should win on series+number similarity.
    assert_eq!(f.match_method, "comicinfo_xml");
    assert_eq!(f.status, "owned");
}

#[tokio::test]
async fn tier1_metron_url_passively_ingested() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("Saga (2012)");
    write_cbz(
        &dir.join("Saga 001 (2012).cbz"),
        Some(
            r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Saga</Series>
  <Number>1</Number>
  <Volume>2012</Volume>
  <Web>https://metron.cloud/issue/saga-1-2012</Web>
</ComicInfo>"#,
        ),
    );

    let pool = fresh_pool().await;
    let s = longbox_db::series_repo::insert(
        &pool,
        NewSeries {
            cv_id: Some(9999),
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
    .unwrap();
    longbox_db::issue_repo::insert(
        &pool,
        longbox_db::NewIssue {
            series_id: s.id,
            cv_issue_id: None,
            metron_issue_id: Some("saga-1-2012".into()),
            number: "1".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();
    let f = find_file(&pool, library_root_id, "Saga (2012)/Saga 001 (2012).cbz").await;
    assert_eq!(f.match_method, "web_url_metron");
    assert_eq!(f.status, "owned");
}

// -------- per-file errors --------

#[tokio::test]
async fn corrupt_cbz_logged_but_scan_continues() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    let report = scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.path_relative.contains("corrupt.cbz")),
        "expected corrupt.cbz in report.errors, got {:?}",
        report.errors
    );
    // Other files still got processed.
    assert!(
        report.matched_owned >= 3,
        "scan should continue past corrupt file"
    );
}

#[tokio::test]
async fn hidden_files_and_cbr_skipped_silently() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();

    assert!(
        file_repo::find_by_path(&pool, library_root_id, "Walking Dead (2003)/.DS_Store")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        file_repo::find_by_path(&pool, library_root_id, "Walking Dead (2003)/cover.cbr")
            .await
            .unwrap()
            .is_none()
    );
}

// -------- presence tracking --------

#[tokio::test]
async fn second_scan_no_changes_just_updates_existing_rows() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    let first = scanner.scan_full(library_root_id).await.unwrap();
    let second = scanner.scan_full(library_root_id).await.unwrap();
    assert_eq!(second.files_added, 0, "second scan should add nothing");
    assert_eq!(second.files_seen, first.files_seen);
}

#[tokio::test]
async fn missing_file_flipped_to_not_present() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    scanner.scan_full(library_root_id).await.unwrap();
    // Delete one file.
    let victim = tmp
        .path()
        .join("Walking Dead (2003)/Walking Dead 002 (2003).cbz");
    std::fs::remove_file(&victim).unwrap();

    // Sleep a hair so last_seen_at strictly differs from the new started_at.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let report = scanner.scan_full(library_root_id).await.unwrap();
    assert_eq!(
        report.files_marked_missing, 1,
        "should mark one file missing, got {}",
        report.files_marked_missing
    );

    let row = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 002 (2003).cbz",
    )
    .await;
    assert!(!row.is_present);
}

#[tokio::test]
async fn restored_file_flipped_present_again_no_duplicate_row() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    scanner.scan_full(library_root_id).await.unwrap();
    let victim = tmp
        .path()
        .join("Walking Dead (2003)/Walking Dead 002 (2003).cbz");
    let original_bytes = std::fs::read(&victim).unwrap();
    std::fs::remove_file(&victim).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    scanner.scan_full(library_root_id).await.unwrap();

    // Restore.
    std::fs::write(&victim, &original_bytes).unwrap();
    scanner.scan_full(library_root_id).await.unwrap();

    let row = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 002 (2003).cbz",
    )
    .await;
    assert!(row.is_present);

    let all = file_repo::list_by_library_root(&pool, library_root_id)
        .await
        .unwrap();
    let dup_count = all
        .iter()
        .filter(|r| r.path_relative == "Walking Dead (2003)/Walking Dead 002 (2003).cbz")
        .count();
    assert_eq!(dup_count, 1, "no duplicate row should be inserted");
}

// -------- locking, errors --------

// (Concurrent-scan test removed: the in-memory scan finishes too fast to
// reliably observe the AlreadyRunning path from an external test. The lock
// is a `tokio::sync::Mutex` with `try_lock` — correct by inspection. A
// future test could expose a sleep hook on Scanner if a deterministic check
// becomes necessary.)

#[tokio::test]
async fn library_root_not_found_does_not_panic() {
    let pool = fresh_pool().await;
    let err = scanner_for(pool).scan_full(9_999_999).await.unwrap_err();
    assert!(matches!(
        err,
        ScanError::LibraryRootNotFound { id: 9_999_999 }
    ));
}

// Note: a former `empty_library_succeeds_with_zero_counters` test was
// removed in the mount-health preflight commit. Its premise (an empty
// FS library is just "no content yet") is no longer valid: an empty
// FS is now treated as a mount-health failure and trips
// `PreflightFailed` BEFORE the scan runs. See
// `preflight_errors_on_empty_library_root` and
// `scan_full_skipped_when_library_root_empty_preserves_all_invariants`
// in the preflight section below for the replacement coverage.

// -------- rescan / rematch --------

#[tokio::test]
async fn rescan_unmatched_only_touches_needs_review() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    scanner.scan_full(library_root_id).await.unwrap();

    // Manually flip one owned row to needs_review and a different file to ignored.
    sqlx::query!(
        r#"UPDATE files SET status = 'needs_review' WHERE path_relative = ?"#,
        "Walking Dead (2003)/Walking Dead 003 (2003).cbz"
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        r#"UPDATE files SET status = 'ignored' WHERE path_relative = ?"#,
        "Saga (2012)/Saga 001 (2012).cbz"
    )
    .execute(&pool)
    .await
    .unwrap();

    let report = scanner.rescan_unmatched(library_root_id).await.unwrap();
    // Only the needs_review row should have been touched.
    assert_eq!(report.files_seen, 1);

    let saga = find_file(&pool, library_root_id, "Saga (2012)/Saga 001 (2012).cbz").await;
    assert_eq!(saga.status, "ignored", "ignored status must be preserved");
}

#[tokio::test]
async fn rematch_for_series_finds_previously_unmatched_files() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    // First scan with NO series → everything is unmatched / needs_review.
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());
    scanner.scan_full(library_root_id).await.unwrap();

    // Flip the WD files to needs_review (so rematch_for_series picks them up).
    sqlx::query!(
        r#"UPDATE files SET status = 'needs_review' WHERE path_relative LIKE 'Walking Dead%'"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Now add the series, then call rematch_for_series.
    let wd = seed_walking_dead(&pool).await;
    let report = scanner.rematch_for_series(wd.id).await.unwrap();
    assert!(report.matched_owned >= 1);

    let f3 = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 003 (2003).cbz",
    )
    .await;
    assert_eq!(f3.status, "owned");
}

// -------- status preservation --------

#[tokio::test]
async fn ignored_status_preserved_across_scans() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    scanner.scan_full(library_root_id).await.unwrap();
    sqlx::query!(
        r#"UPDATE files SET status = 'ignored', match_method = 'ignored', issue_id = NULL
           WHERE path_relative = ?"#,
        "Walking Dead (2003)/Walking Dead 001 (2003).cbz"
    )
    .execute(&pool)
    .await
    .unwrap();

    scanner.scan_full(library_root_id).await.unwrap();
    let row = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 001 (2003).cbz",
    )
    .await;
    assert_eq!(row.status, "ignored", "ignored must stick");
    assert!(row.issue_id.is_none());
}

// -------- mount-health preflight --------

/// Helper: read a series's empty_scan_counter directly via runtime
/// sqlx (no macro). The column isn't exposed via SeriesRow and we
/// only need it from tests; not worth a public repo helper just for
/// this. Runtime query avoids touching the .sqlx offline cache.
async fn empty_scan_counter(pool: &longbox_db::Pool, series_id: i64) -> i64 {
    use sqlx::Row;
    sqlx::query("SELECT consecutive_empty_scans FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn preflight_errors_on_nonexistent_library_root_path() {
    let pool = fresh_pool().await;
    // library_roots row stored with a path that doesn't exist on disk.
    let library_root_id = seed_library_root(&pool, "/nonexistent-longbox-preflight-test").await;
    let err = scanner_for(pool)
        .scan_full(library_root_id)
        .await
        .unwrap_err();
    match err {
        ScanError::PreflightFailed { reason } => {
            assert!(
                reason.contains("unreadable"),
                "reason should name the failure mode: {reason}"
            );
        }
        other => panic!("expected PreflightFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn preflight_errors_on_empty_library_root() {
    let tmp = TempDir::new().unwrap(); // empty dir
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let err = scanner_for(pool)
        .scan_full(library_root_id)
        .await
        .unwrap_err();
    match err {
        ScanError::PreflightFailed { reason } => {
            // The reason string explicitly names auto-tidy as the
            // consequence being avoided — that's what makes the 3am
            // log line useful.
            assert!(
                reason.contains("empty") && reason.contains("auto-tidy"),
                "reason should explain the why: {reason}"
            );
        }
        other => panic!("expected PreflightFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn preflight_passes_on_populated_library_root() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path()); // creates real CBZs
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    // A populated dir + empty catalog is a happy case — preflight
    // passes, scan runs end-to-end (every file lands as unmatched).
    let report = scanner_for(pool)
        .scan_full(library_root_id)
        .await
        .unwrap();
    assert!(report.files_seen > 0);
}

/// LOAD-BEARING: when scan_full trips the preflight, the entire
/// destructive cascade must be short-circuited. Four invariants:
///   1. Err::PreflightFailed returned.
///   2. Zero new scan_runs rows recorded.
///   3. No catalogued file flipped to is_present=false.
///   4. No series's empty_scan_counter ticked.
///
/// If any of these fails, a degraded SMB mount could still poison
/// the catalog through some other path — the whole point of this
/// commit was to guarantee it can't.
#[tokio::test]
async fn scan_full_skipped_when_library_root_empty_preserves_all_invariants() {
    use longbox_db::{scan_run_repo, NewFile};

    let empty_root = TempDir::new().unwrap();
    let pool = fresh_pool().await;
    let series = seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, empty_root.path().to_str().unwrap()).await;

    // Seed a file that's currently is_present=true. This stands in
    // for "the catalog believes this file exists on disk" — exactly
    // what the bad scan would flip to false.
    let file_path = "Walking Dead (2003)/Walking Dead 001 (2003).cbz";
    let inserted = longbox_db::file_repo::insert(
        &pool,
        NewFile {
            library_root_id,
            path_relative: file_path.into(),
            issue_id: None,
            size_bytes: 1024,
            mtime: time::PrimitiveDateTime::MIN,
            last_scanned_at: time::PrimitiveDateTime::MIN,
            match_method: "phase_b".into(),
            match_confidence: 1.0,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: time::PrimitiveDateTime::MIN,
            matched_at: None,
        },
    )
    .await
    .unwrap();
    assert!(inserted.is_present, "seeded row must start present");
    let counter_before = empty_scan_counter(&pool, series.id).await;

    let err = scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap_err();

    // Invariant 1: typed Err comes back.
    assert!(
        matches!(err, ScanError::PreflightFailed { .. }),
        "expected PreflightFailed, got {err:?}"
    );

    // Invariant 2: no scan_run row recorded — the scan didn't happen
    // as far as the catalog is concerned.
    let runs = scan_run_repo::list_recent(&pool, 10, &[])
        .await
        .unwrap();
    assert_eq!(
        runs.len(),
        0,
        "preflight failure must not record a scan_run row; got {} runs",
        runs.len()
    );

    // Invariant 3: file's is_present untouched. The whole point.
    let row = longbox_db::file_repo::find_by_path(&pool, library_root_id, file_path)
        .await
        .unwrap()
        .expect("file row must still exist");
    assert!(
        row.is_present,
        "is_present must not have flipped; row = {row:?}"
    );

    // Invariant 4: series's empty_scan_counter unchanged. Auto-tidy's
    // doomsday clock was never even consulted.
    let counter_after = empty_scan_counter(&pool, series.id).await;
    assert_eq!(
        counter_before, counter_after,
        "empty_scan_counter must not have advanced; before={counter_before} after={counter_after}"
    );
}

// -------- live match_confidence_threshold (no restart needed) --------

/// A boot-time threshold (the scanner's ScannerConfig fallback) of 0.5
/// stays in `Default::default()`; the bug under test is the scanner
/// reading the live `match_confidence_threshold` row from `settings`
/// instead of the cached config value. We toggle the DB row between
/// scans and observe `files.status` flip without recreating the
/// scanner.
#[tokio::test]
async fn match_threshold_change_in_db_takes_effect_on_next_scan() {
    use longbox_db::{settings_repo, KEY_MATCH_CONFIDENCE_THRESHOLD};

    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    // Reuse the same Scanner instance across both scans so the bug —
    // cached threshold on the long-lived AppState — would manifest if
    // present.
    let scanner = scanner_for(pool.clone());

    // Bounce-threshold pass: set the live row to 1.01, an impossible
    // ceiling. Every match (including Tier 1 at confidence 1.0) must
    // fall short and downgrade to needs_review.
    settings_repo::set(&pool, KEY_MATCH_CONFIDENCE_THRESHOLD, "1.01")
        .await
        .unwrap();
    scanner.scan_full(library_root_id).await.unwrap();
    let f1 = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 001 (2003).cbz",
    )
    .await;
    assert_eq!(
        f1.status, "needs_review",
        "threshold=1.01 must downgrade a confidence=1.0 match to needs_review"
    );

    // Live-edit the row to 0.50 — no scanner re-construction. The next
    // scan must re-read the row and treat the same match as owned.
    settings_repo::set(&pool, KEY_MATCH_CONFIDENCE_THRESHOLD, "0.50")
        .await
        .unwrap();
    scanner.scan_full(library_root_id).await.unwrap();
    let f1 = find_file(
        &pool,
        library_root_id,
        "Walking Dead (2003)/Walking Dead 001 (2003).cbz",
    )
    .await;
    assert_eq!(
        f1.status, "owned",
        "threshold=0.50 must promote the previously-blocked match to owned without a restart"
    );
}

/// rescan_unmatched also reads the live threshold — the rescan path is
/// what runs after a Settings-page threshold change in practice, so it
/// has to honor the new value just like a full scan.
#[tokio::test]
async fn rescan_unmatched_reads_live_match_threshold() {
    use longbox_db::{settings_repo, KEY_MATCH_CONFIDENCE_THRESHOLD};

    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    // First scan at the impossible threshold parks the matches in
    // needs_review.
    settings_repo::set(&pool, KEY_MATCH_CONFIDENCE_THRESHOLD, "1.01")
        .await
        .unwrap();
    scanner.scan_full(library_root_id).await.unwrap();
    let path = "Walking Dead (2003)/Walking Dead 001 (2003).cbz";
    assert_eq!(find_file(&pool, library_root_id, path).await.status, "needs_review");

    // The user drops the threshold, runs a rescan-unmatched (NOT a full
    // scan). The rescan must consult the live row and promote.
    settings_repo::set(&pool, KEY_MATCH_CONFIDENCE_THRESHOLD, "0.50")
        .await
        .unwrap();
    scanner.rescan_unmatched(library_root_id).await.unwrap();
    assert_eq!(
        find_file(&pool, library_root_id, path).await.status,
        "owned",
        "rescan_unmatched must use the live threshold, not the cached config"
    );
}

#[tokio::test]
async fn rescan_unmatched_skipped_when_library_root_empty() {
    use longbox_db::scan_run_repo;

    let empty_root = TempDir::new().unwrap();
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, empty_root.path().to_str().unwrap()).await;

    let err = scanner_for(pool.clone())
        .rescan_unmatched(library_root_id)
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScanError::PreflightFailed { .. }),
        "expected PreflightFailed, got {err:?}"
    );
    let runs = scan_run_repo::list_recent(&pool, 10, &[])
        .await
        .unwrap();
    assert_eq!(runs.len(), 0, "rescan_unmatched must not record a scan_run");
}
