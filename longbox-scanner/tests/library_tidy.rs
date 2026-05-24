//! Library Tidy Step 2 — scanner reconciliation: discovered-folder
//! detection and the phantom-transition signal (`last_matched_count`).

mod common;

use common::{build_fixture_library, fresh_pool, seed_library_root, seed_walking_dead, write_cbz};
use longbox_db::{discovered_folders_repo, pull_list_repo, series_repo, NewPullEntry, PhantomSeries};
use longbox_scanner::{Scanner, ScannerConfig};
use tempfile::TempDir;

fn scanner_for(db: longbox_db::Pool) -> Scanner {
    Scanner::new(db, ScannerConfig::default())
}

/// The phantom row for `series_id` — fails the test if the series is
/// not currently a phantom.
async fn phantom(pool: &longbox_db::Pool, series_id: i64) -> PhantomSeries {
    series_repo::list_phantoms(pool)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == series_id)
        .expect("series is a phantom")
}

async fn discovered_names(pool: &longbox_db::Pool) -> Vec<String> {
    discovered_folders_repo::list(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.folder_name)
        .collect()
}

#[tokio::test]
async fn untracked_folders_are_discovered() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    // No series seeded — every top-level folder resolves to nothing.
    scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();

    let names = discovered_names(&pool).await;
    assert!(names.contains(&"Walking Dead (2003)".to_string()));
    assert!(names.contains(&"Saga (2012)".to_string()));
    assert!(names.contains(&"Mystery".to_string()));
}

#[tokio::test]
async fn a_tracked_series_folder_is_not_discovered() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();

    let names = discovered_names(&pool).await;
    // The Walking Dead folder's files resolve to the tracked series.
    assert!(!names.contains(&"Walking Dead (2003)".to_string()));
    // The untracked folders still surface.
    assert!(names.contains(&"Saga (2012)".to_string()));
    assert!(names.contains(&"Mystery".to_string()));
}

#[tokio::test]
async fn a_dismissed_folder_is_not_rediscovered() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    scanner.scan_full(library_root_id).await.unwrap();
    discovered_folders_repo::dismiss(&pool, &["Mystery".into()])
        .await
        .unwrap();

    // A second scan re-detects the folder, but the upsert leaves a
    // dismissed row dismissed.
    scanner.scan_full(library_root_id).await.unwrap();
    assert!(!discovered_names(&pool)
        .await
        .contains(&"Mystery".to_string()));
}

#[tokio::test]
async fn a_series_that_loses_its_files_keeps_its_last_matched_count() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let wd = seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    // Scan 1: the Walking Dead folder is on disk and matches.
    scanner.scan_full(library_root_id).await.unwrap();

    // The user deletes the folder; scan 2 marks the files missing.
    std::fs::remove_dir_all(tmp.path().join("Walking Dead (2003)")).unwrap();
    scanner.scan_full(library_root_id).await.unwrap();

    // The series is now a phantom — and a *transition* one: its
    // last_matched_count was retained from scan 1, not zeroed.
    let phantoms = series_repo::list_phantoms(&pool).await.unwrap();
    let row = phantoms
        .iter()
        .find(|p| p.id == wd.id)
        .expect("Walking Dead is a phantom after losing its files");
    assert!(
        row.last_matched_count > 0,
        "a series that just lost its files retains last_matched_count > 0 (transition signal)"
    );
}

#[tokio::test]
async fn an_existing_discovered_folder_refreshes_its_file_count_on_rescan() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    // Scan 1: the `Mystery` folder holds a single CBZ.
    scanner.scan_full(library_root_id).await.unwrap();
    let count_of = |rows: &[longbox_db::DiscoveredFolderRow]| {
        rows.iter()
            .find(|r| r.folder_name == "Mystery")
            .expect("Mystery is discovered")
            .file_count
    };
    let before = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(count_of(&before), 1);

    // A second CBZ lands in the folder; scan 2 re-detects it and the
    // upsert's existing-row branch must refresh `file_count`.
    write_cbz(&tmp.path().join("Mystery/SecondComic.cbz"), None);
    scanner.scan_full(library_root_id).await.unwrap();

    let after = discovered_folders_repo::list(&pool).await.unwrap();
    assert_eq!(
        count_of(&after),
        2,
        "re-detecting an existing folder refreshes its file_count"
    );
}

#[tokio::test]
async fn a_never_owned_series_is_a_steady_state_phantom() {
    // Empty library — the seeded series never has files on disk.
    let tmp = TempDir::new().unwrap();
    let pool = fresh_pool().await;
    let wd = seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;

    scanner_for(pool.clone())
        .scan_full(library_root_id)
        .await
        .unwrap();

    let phantoms = series_repo::list_phantoms(&pool).await.unwrap();
    let row = phantoms
        .iter()
        .find(|p| p.id == wd.id)
        .expect("a series with no files is a phantom");
    assert_eq!(
        row.last_matched_count, 0,
        "a series that never owned a file stays at last_matched_count 0 (steady-state)"
    );
}

// -------- auto-tidy on folder removal (A.9 Step 6b) --------

#[tokio::test]
async fn a_series_is_marked_for_auto_tidy_after_repeated_empty_scans() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let wd = seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    // Scan 1: the folder is on disk — the series owns files, unmarked.
    scanner.scan_full(library_root_id).await.unwrap();

    // The user deletes the folder. Each subsequent scan finds the series
    // empty; two empty scans is still below the threshold of 3.
    std::fs::remove_dir_all(tmp.path().join("Walking Dead (2003)")).unwrap();
    for _ in 0..2 {
        scanner.scan_full(library_root_id).await.unwrap();
    }
    assert!(
        phantom(&pool, wd.id).await.auto_tidy_due_at.is_none(),
        "two empty scans is below the auto-tidy threshold"
    );

    // The third empty scan crosses the threshold and marks the series.
    let report = scanner.scan_full(library_root_id).await.unwrap();
    assert_eq!(report.series_auto_marked, 1);
    assert!(
        phantom(&pool, wd.id).await.auto_tidy_due_at.is_some(),
        "the third empty scan marks the series for automatic removal"
    );
}

#[tokio::test]
async fn a_pull_list_series_is_never_auto_marked() {
    // Empty library — the seeded series never has files on disk.
    let tmp = TempDir::new().unwrap();
    let pool = fresh_pool().await;
    let wd = seed_walking_dead(&pool).await;
    pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id: wd.id,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    // Several empty scans, well past the threshold.
    for _ in 0..5 {
        let report = scanner.scan_full(library_root_id).await.unwrap();
        assert_eq!(report.series_auto_marked, 0);
    }
    assert!(
        phantom(&pool, wd.id).await.auto_tidy_due_at.is_none(),
        "a pull-list series is awaiting a first download — never auto-marked"
    );
}

#[tokio::test]
async fn a_marked_series_is_recovered_when_its_folder_returns() {
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let wd = seed_walking_dead(&pool).await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    scanner.scan_full(library_root_id).await.unwrap();
    std::fs::remove_dir_all(tmp.path().join("Walking Dead (2003)")).unwrap();
    for _ in 0..3 {
        scanner.scan_full(library_root_id).await.unwrap();
    }
    assert!(
        phantom(&pool, wd.id).await.auto_tidy_due_at.is_some(),
        "three empty scans mark the series"
    );

    // The folder reappears before the recovery window elapses.
    build_fixture_library(tmp.path());
    scanner.scan_full(library_root_id).await.unwrap();

    // The series owns files again, so it is no longer a phantom — the
    // empty-scan counter and the removal mark were both cleared.
    assert!(
        series_repo::list_phantoms(&pool)
            .await
            .unwrap()
            .iter()
            .all(|p| p.id != wd.id),
        "a series whose folder returned is recovered, not a phantom"
    );
}

// -------- A.9 hot-fix item F6: discovered_folders staleness --------

#[tokio::test]
async fn a_discovered_folder_is_auto_dismissed_once_its_files_resolve() {
    // Scan once with no series seeded → the Walking Dead folder shows
    // as untracked. Add the matching series → next scan's cascade
    // resolves its files; the `discovered_folders` row is auto-
    // dismissed. Without F6 it would stay in /api/reconcile/untracked
    // forever — the proximate cause of bulk-convert dupe candidates
    // pre-hot-fix.
    let tmp = TempDir::new().unwrap();
    build_fixture_library(tmp.path());
    let pool = fresh_pool().await;
    let library_root_id = seed_library_root(&pool, tmp.path().to_str().unwrap()).await;
    let scanner = scanner_for(pool.clone());

    scanner.scan_full(library_root_id).await.unwrap();
    let before: Vec<String> = discovered_folders_repo::list(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.folder_name)
        .collect();
    assert!(before.contains(&"Walking Dead (2003)".to_string()));

    // Add the series so the fixture's files now resolve to it.
    seed_walking_dead(&pool).await;
    let report = scanner.scan_full(library_root_id).await.unwrap();
    assert!(
        report.folders_auto_dismissed >= 1,
        "the scanner reports the auto-dismiss count"
    );

    let after: Vec<String> = discovered_folders_repo::list(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.folder_name)
        .collect();
    assert!(
        !after.contains(&"Walking Dead (2003)".to_string()),
        "the previously-untracked folder is auto-dismissed once its files resolve"
    );
}
