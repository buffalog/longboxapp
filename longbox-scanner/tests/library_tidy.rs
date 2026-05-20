//! Library Tidy Step 2 — scanner reconciliation: discovered-folder
//! detection and the phantom-transition signal (`last_matched_count`).

mod common;

use common::{build_fixture_library, fresh_pool, seed_library_root, seed_walking_dead};
use longbox_db::{discovered_folders_repo, series_repo};
use longbox_scanner::{Scanner, ScannerConfig};
use tempfile::TempDir;

fn scanner_for(db: longbox_db::Pool) -> Scanner {
    Scanner::new(db, ScannerConfig::default())
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
