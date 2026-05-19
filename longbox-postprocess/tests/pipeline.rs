//! End-to-end Step 6 verification: seed a catalog, drop a real CBZ
//! into a tempdir, run the full pipeline, assert the file moved + the
//! catalog row landed.
//!
//! Bypasses the watcher entry-point and calls `process_one` directly
//! so the test doesn't have to race notify's event delivery. The
//! watcher → consumer → process_one wiring is covered by the live
//! tests in `tests/live_detection.rs` (Step 5).

use std::io::{Read, Write};
use std::path::Path;

use longbox_db::{
    issue_repo, library_root_repo, series_repo, NewIssue, NewLibraryRoot, NewSeries, Pool,
};
use longbox_postprocess::processor::{self, Outcome};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

struct Fixture {
    _watch: TempDir,
    library: TempDir,
    db: Pool,
    library_root_id: i64,
    series_id: i64,
    issue_id: i64,
}

async fn seed_basic_fixture() -> Fixture {
    let db = longbox_db::open(":memory:").await.unwrap();
    let watch = TempDir::new().unwrap();
    let library = TempDir::new().unwrap();

    let library_root_id = library_root_repo::insert(
        &db,
        NewLibraryRoot {
            path: library.path().to_string_lossy().into_owned(),
        },
    )
    .await
    .unwrap()
    .id;
    let series_id = series_repo::insert(
        &db,
        NewSeries {
            cv_id: Some(100),
            metron_id: None,
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2012),
            publisher: Some("Image".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        &db,
        NewIssue {
            series_id,
            cv_issue_id: Some(364354),
            metron_issue_id: None,
            number: "1".into(),
            title: Some("The Will".into()),
            cover_date: Some("2012-03-14".into()),
            summary: Some("<p>Galactic war epic.</p>".into()),
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    Fixture {
        _watch: watch,
        library,
        db,
        library_root_id,
        series_id,
        issue_id,
    }
}

fn write_cbz(path: &Path, comic_info: Option<&str>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let file = std::fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("page-001.jpg", opts).unwrap();
    zip.write_all(b"\xFF\xD8\xFF\xE0\x00\x10JFIF").unwrap();
    if let Some(xml) = comic_info {
        zip.start_file("ComicInfo.xml", opts).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    // Push mtime back so the stability check doesn't sleep — tests
    // measure end-to-end behavior, not the 2 s wait.
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(earlier))
        .ok(); // best-effort; if filetime isn't a dep, sleep instead
}

fn read_cbz_entry(path: &Path, entry_name: &str) -> Option<String> {
    let file = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        if entry.name().eq_ignore_ascii_case(entry_name) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).unwrap();
            return String::from_utf8(buf).ok();
        }
    }
    None
}

fn list_cbz_entries(path: &Path) -> Vec<String> {
    let file = std::fs::File::open(path).unwrap();
    let archive = zip::ZipArchive::new(file).unwrap();
    archive.file_names().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn imports_owned_via_filename_match() {
    let f = seed_basic_fixture().await;

    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
    )
    .await
    .unwrap();

    let target = match outcome {
        Outcome::Imported { target, series_id, issue_id, .. } => {
            assert_eq!(series_id, f.series_id);
            assert_eq!(issue_id, f.issue_id);
            target
        }
        other => panic!("expected Imported, got {other:?}"),
    };
    assert!(target.exists(), "target file should exist at {target:?}");
    assert!(!source.exists(), "source should be removed after import");

    // Target lives at the convention-driven library path.
    let expected_path = f
        .library
        .path()
        .join("Saga (2012)")
        .join("Saga (2012) 001.cbz");
    assert_eq!(target, expected_path);

    // ComicInfo.xml was written into the target.
    let xml = read_cbz_entry(&target, "ComicInfo.xml").expect("ComicInfo.xml missing");
    assert!(xml.contains("<Series>Saga</Series>"));
    assert!(xml.contains("<Number>1</Number>"));
    assert!(xml.contains("<Year>2012</Year>"));
    assert!(xml.contains("<Web>https://comicvine.gamespot.com/issue/4000-364354/</Web>"));

    // Catalog row reflects Phase B's import.
    let row = longbox_db::file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(row.issue_id, Some(f.issue_id));
    assert_eq!(row.status, "owned");
    assert_eq!(row.match_method, "phase_b");
    assert!((row.match_confidence - 1.0).abs() < f64::EPSILON);
    assert!(row.is_present);
    assert!(row.matched_at.is_some());
}

#[tokio::test]
async fn overwrites_existing_comicinfo_in_source() {
    let f = seed_basic_fixture().await;

    let stale_xml = r#"<?xml version="1.0"?><ComicInfo><Series>Stale Data</Series><Number>999</Number></ComicInfo>"#;
    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, Some(stale_xml));

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db)
        .await
        .unwrap();
    let target = match outcome {
        Outcome::Imported { target, .. } => target,
        other => panic!("expected Imported, got {other:?}"),
    };

    let xml = read_cbz_entry(&target, "ComicInfo.xml").unwrap();
    assert!(
        !xml.contains("Stale Data"),
        "old ComicInfo leaked through: {xml}"
    );
    assert!(xml.contains("<Series>Saga</Series>"));
    // Only one ComicInfo.xml entry in the archive.
    let entries = list_cbz_entries(&target);
    let count = entries
        .iter()
        .filter(|n| n.eq_ignore_ascii_case("ComicInfo.xml"))
        .count();
    assert_eq!(count, 1, "exactly one ComicInfo.xml expected, got {entries:?}");
}

#[tokio::test]
async fn unmatched_file_lands_in_unsorted() {
    let f = seed_basic_fixture().await;

    // Filename doesn't match the seeded series. The matcher returns
    // Unmatched; the file should move to _unsorted/.
    let source = f._watch.path().join("Totally Unknown 042.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db)
        .await
        .unwrap();
    let target = match outcome {
        Outcome::Unsorted { target, .. } => target,
        other => panic!("expected Unsorted, got {other:?}"),
    };

    let expected = f.library.path().join("_unsorted").join("Totally Unknown 042.cbz");
    assert_eq!(target, expected);
    assert!(target.exists());
    assert!(!source.exists());

    let row = longbox_db::file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "_unsorted/Totally Unknown 042.cbz",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(row.status, "unmatched");
    assert_eq!(row.match_method, "phase_b");
    assert_eq!(row.issue_id, None);
    assert!(row.matched_at.is_none());
}

#[tokio::test]
async fn conflict_leaves_source_untouched() {
    let f = seed_basic_fixture().await;

    // Pre-place a file at the target location.
    let target_dir = f.library.path().join("Saga (2012)");
    std::fs::create_dir_all(&target_dir).unwrap();
    let existing_target = target_dir.join("Saga (2012) 001.cbz");
    std::fs::write(&existing_target, b"pre-existing").unwrap();

    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db)
        .await
        .unwrap();
    match outcome {
        Outcome::Conflict { target, .. } => assert_eq!(target, existing_target),
        other => panic!("expected Conflict, got {other:?}"),
    }

    assert!(source.exists(), "source must stay put on conflict");
    let existing_bytes = std::fs::read(&existing_target).unwrap();
    assert_eq!(existing_bytes, b"pre-existing", "target must not be overwritten");

    // No catalog row was written for the source.
    let row = longbox_db::file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
    )
    .await
    .unwrap();
    assert!(row.is_none(), "no catalog row expected on conflict");
}

#[tokio::test]
async fn idempotent_reprocessing() {
    // Drop the same file twice — the second pass should be a no-op
    // from the user's perspective (same target, same catalog row).
    // First pass moves source → target, so the test re-creates the
    // source for the second pass.
    let f = seed_basic_fixture().await;

    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);
    let first = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db)
        .await
        .unwrap();
    let target = match first {
        Outcome::Imported { target, .. } => target,
        _ => panic!(),
    };

    // Reprocess the target itself: it already lives in the library
    // and has a catalog row. process_one should detect the conflict
    // (target exists at its own path).
    let outcome2 = processor::process_one(&target, f.library.path(), f.library_root_id, &f.db)
        .await
        .unwrap();
    // Already-at-target reprocessing surfaces as Conflict (the target
    // path is the source path, so target.exists() is true). The
    // important property: no destructive change.
    assert!(matches!(outcome2, Outcome::Conflict { .. }));
    assert!(target.exists());
}

#[tokio::test]
async fn rewrite_failure_surfaces_as_outcome_failed() {
    // Engineer a deterministic stage-1 (rewrite) failure: pre-create a
    // regular file where the series subdirectory should go. `create_dir_all`
    // inside rewrite_to_temp will fail because the path exists as a file,
    // and the failure must surface as Outcome::Failed{ ComicInfoWriteFailed }
    // — not as Err, not as Conflict (target_abs itself doesn't exist).
    let f = seed_basic_fixture().await;

    // Library convention: library_root/Saga (2012)/Saga (2012) 001.cbz
    // Block the directory creation by planting a regular file at
    // library_root/Saga (2012).
    let blocker = f.library.path().join("Saga (2012)");
    std::fs::write(&blocker, b"blocking the directory").unwrap();

    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db)
        .await
        .unwrap();

    match outcome {
        Outcome::Failed { reason, size, target } => {
            use longbox_postprocess::InterventionReason;
            assert!(
                matches!(reason, InterventionReason::ComicInfoWriteFailed(_)),
                "stage-1 failure should be ComicInfoWriteFailed, got {reason:?}"
            );
            assert!(size > 0, "size should reflect source file");
            assert!(
                target.to_string_lossy().contains("Saga (2012)"),
                "target should point at the intended library path: {}",
                target.display()
            );
        }
        other => panic!("expected Outcome::Failed, got {other:?}"),
    }

    // Source must stay in the watch folder on failure — that's the
    // whole point of pending intervention. User decides what to do.
    assert!(source.exists(), "source must remain on failure");

    // No catalog row was written.
    let row = longbox_db::file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
    )
    .await
    .unwrap();
    assert!(row.is_none(), "no catalog row expected on Outcome::Failed");

    // Suppress unused-field warnings for fixture handles.
    let _ = (f.series_id, f.issue_id);
}
