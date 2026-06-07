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
use longbox_postprocess::PendingInterventionsCache;
use std::sync::Arc;
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
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(earlier)).ok();
    // best-effort; if filetime isn't a dep, sleep instead
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
async fn sweep_now_tallies_outcomes_across_the_watch_folder() {
    // The Settings page's "Process downloads" button hits
    // `sweep_now` directly. Cover the per-bucket tally and the
    // self-consistency invariant: total visited files == sum of
    // per-bucket counters.
    let f = seed_basic_fixture().await;
    // Stub CBZs are 200 bytes; disable the 35 MB size floor so the
    // test exercises the normal import path. See the dedicated
    // RejectedTooSmall test for size-floor coverage.
    longbox_db::settings_repo::set(&f.db, "min_file_size_mb", "0")
        .await
        .unwrap();

    // Matched → Imported. Filename parses to Saga #1, which the
    // fixture's series + issue match exactly.
    let owned = f._watch.path().join("Saga 001.cbz");
    write_cbz(&owned, None);

    // Unparseable basename → Skipped (no usable title hint). The file
    // stays in /watch/; no DB write, no disk move.
    let mystery = f._watch.path().join("garbage_no_number.cbz");
    write_cbz(&mystery, None);

    // Conflict: pre-place the canonical target so the second-claim
    // path fires, source auto-cleaned.
    let conflict_target_dir = f.library.path().join("Saga (2012)");
    std::fs::create_dir_all(&conflict_target_dir).unwrap();
    let conflict_target = conflict_target_dir.join("Saga (2012) 002.cbz");
    std::fs::write(&conflict_target, b"pre-existing").unwrap();
    let conflict_source = f._watch.path().join("Saga 002.cbz");
    write_cbz(&conflict_source, None);
    // Seed the second issue in the catalog so the conflict-path
    // resolution actually finds a target — without an issue row,
    // Saga 002 would have nothing to claim and lands as Unsorted.
    issue_repo::insert(
        &f.db,
        NewIssue {
            series_id: f.series_id,
            cv_issue_id: Some(364355),
            metron_issue_id: None,
            number: "2".into(),
            title: None,
            cover_date: Some("2012-04-14".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    let cache = Arc::new(PendingInterventionsCache::new());
    let summary = longbox_postprocess::sweep_now(
        f.library.path(),
        f.library_root_id,
        f._watch.path(),
        f.db.clone(),
        Arc::clone(&cache),
    )
    .await
    .unwrap();

    assert_eq!(summary.processed, 1, "Saga 001 → Imported");
    assert_eq!(summary.skipped, 1, "garbage_no_number → Skipped");
    assert_eq!(summary.conflicts, 1, "Saga 002 → Conflict (target exists)");
    assert_eq!(summary.failed, 0);

    // Side-effect checks: matched file moved, conflict source cleaned
    // up (Phase B-bug-2 invariant), target bytes preserved, and the
    // Skipped file stays in /watch/ untouched (per Jeremy's directive
    // — the watch folder is the holding pen, not _unsorted/).
    assert!(!owned.exists(), "matched source must move out of watch");
    assert!(!conflict_source.exists(), "conflict source must be auto-removed");
    assert!(
        mystery.exists(),
        "Skipped source must stay in /watch/ — no _unsorted/ parking lot"
    );
    assert_eq!(
        std::fs::read(&conflict_target).unwrap(),
        b"pre-existing",
        "library bytes must not be overwritten"
    );

    // Cache: Conflict no longer pushes (it's resolved), Imported and
    // Unsorted evict — so the cache lands empty for this run.
    assert!(cache.is_empty(), "no pending interventions expected");
}

#[tokio::test]
async fn sweep_now_400s_when_watch_path_is_missing() {
    let f = seed_basic_fixture().await;
    let nonexistent = f.library.path().join("does-not-exist");
    let cache = Arc::new(PendingInterventionsCache::new());
    let err = longbox_postprocess::sweep_now(
        f.library.path(),
        f.library_root_id,
        &nonexistent,
        f.db.clone(),
        cache,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            longbox_postprocess::PostprocessError::WatchPathUnreadable { .. }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn imports_owned_via_dot_separated_nzb_style_basename() {
    // Regression for the Phase B parser bug: scene/NZB-style
    // basenames (dots instead of spaces, parenthesized year + tags)
    // must parse via the dot-to-space cascade and land as owned,
    // NOT in `_unsorted/`. The user-visible example:
    // `Absolute.Green.Lantern.007.(2025).(Digital).(Shan-Empire).cbz`
    // — same shape with the fixture's Saga catalog.
    let f = seed_basic_fixture().await;

    let source = f
        ._watch
        .path()
        .join("Saga.001.(2012).(Digital).(Empire).cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();
    match outcome {
        Outcome::Imported {
            series_id,
            issue_id,
            target,
            ..
        } => {
            assert_eq!(series_id, f.series_id, "must attribute to the seeded series");
            assert_eq!(issue_id, f.issue_id, "must attribute to issue #1");
            // Target lands at the canonical path — the dot-separated
            // source is moved into the convention-driven library
            // location.
            let expected = f
                .library
                .path()
                .join("Saga (2012)")
                .join("Saga (2012) 001.cbz");
            assert_eq!(target, expected);
        }
        other => panic!(
            "dot-separated NZB name must import as owned (got {other:?}); \
             the normalizer cascade in Phase B's processor isn't routing"
        ),
    }
    assert!(
        !source.exists(),
        "dot-separated source must move out of /watch on owned import"
    );
}

#[tokio::test]
async fn tier1_folder_name_match_overrides_unparseable_scene_filename() {
    // The load-bearing redesign: SAB drops one job folder per
    // download. The folder name IS the search query LongBox sent the
    // indexer (`Saga 001`), so it faithfully encodes series + issue.
    // The scene group then renames the INNER file to whatever
    // (`Sga.001.(2012).(Digital).(Zone-Empire).cbr` — `Sga` not
    // `Saga`, deliberately broken to make sure Tier 2 cannot rescue
    // this case). Tier 1's folder-name match catches it via the
    // folder; Tier 2's filename parse would skip.
    let f = seed_basic_fixture().await;
    let job_folder = f._watch.path().join("Saga 001");
    std::fs::create_dir_all(&job_folder).unwrap();
    // Inner file uses `.cbz` so `write_cbz`'s ZIP bytes land at an
    // extension Phase B's archive reader can open. The Tier 1 fix
    // is about the FOLDER NAME being authoritative regardless of
    // what the inner filename says; the extension is incidental.
    let source = job_folder.join("Sga.001.(2012).(Digital).(Zone-Empire).cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await
    .unwrap();
    match outcome {
        Outcome::Imported {
            series_id,
            issue_id,
            target,
            ..
        } => {
            assert_eq!(series_id, f.series_id);
            assert_eq!(issue_id, f.issue_id);
            assert_eq!(
                target,
                f.library
                    .path()
                    .join("Saga (2012)")
                    .join("Saga (2012) 001.cbz")
            );
        }
        other => panic!("expected Imported via Tier 1 folder match, got {other:?}"),
    }
    assert!(!source.exists(), "source must move to library");
}

#[tokio::test]
async fn tier1_handles_bare_folder_name_with_no_year() {
    // Folder name shape the user called out: `{title} {issue}` with
    // no year, no parens, no extension. Parses cleanly via id=4
    // catch-all once the cascade appends `.cbz`. Synthetic series
    // because the fixture's Saga has start_year=2012 and the
    // bare-form test wants to prove the year-less path works.
    let f = seed_basic_fixture().await;
    longbox_db::series_repo::insert(
        &f.db,
        longbox_db::NewSeries {
            cv_id: Some(424242),
            metron_id: None,
            title: "Y The Last Man".into(),
            sort_title: "y the last man".into(),
            start_year: Some(2002),
            publisher: Some("DC".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    let issue = longbox_db::issue_repo::insert(
        &f.db,
        longbox_db::NewIssue {
            series_id: 2,
            cv_issue_id: Some(424201),
            metron_issue_id: None,
            number: "42".into(),
            title: None,
            cover_date: Some("2006-02-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    let job_folder = f._watch.path().join("Y The Last Man 42");
    std::fs::create_dir_all(&job_folder).unwrap();
    // Inner file is also bare — both shapes must parse. Uses
    // `.cbz` to match the ZIP bytes `write_cbz` writes; the parser
    // accepts either extension via id=4's `(?i:cbz|cbr|cb7)`.
    let source = job_folder.join("Y The Last Man 42.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await
    .unwrap();
    match outcome {
        Outcome::Imported { issue_id, .. } => {
            assert_eq!(issue_id, issue.id, "must attribute to Y The Last Man #42");
        }
        other => panic!("bare folder + bare filename must import; got {other:?}"),
    }
}

#[tokio::test]
async fn tier1_falls_through_to_tier2_when_folder_parse_misses() {
    // File dropped DIRECTLY in /watch/ — `source.parent()` is the
    // watch root, whose name ("watch", "complete", etc.) is not a
    // catalog series. Tier 1 returns None; Tier 2's filename parse
    // does the actual matching.
    let f = seed_basic_fixture().await;
    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await
    .unwrap();
    match outcome {
        Outcome::Imported {
            series_id,
            issue_id,
            ..
        } => {
            assert_eq!(series_id, f.series_id);
            assert_eq!(issue_id, f.issue_id);
        }
        other => panic!(
            "Tier 2 fallback must still work when Tier 1 has no folder context; got {other:?}"
        ),
    }
}

#[tokio::test]
async fn imports_owned_via_filename_match() {
    let f = seed_basic_fixture().await;

    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();

    let target = match outcome {
        Outcome::Imported {
            target,
            series_id,
            issue_id,
            ..
        } => {
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
async fn imports_owned_cbr_converting_to_cbz() {
    let f = seed_basic_fixture().await;

    // A real CBR (RAR5) carrying ComicInfo for Saga #1. There is no
    // Rust RAR writer, so the fixture is a committed binary copied in
    // rather than built in-process like write_cbz does for CBZ.
    let source = f._watch.path().join("Saga 001.cbr");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-rar5.cbr");
    std::fs::copy(&fixture, &source).unwrap();
    // Push mtime back so the 2 s stability check doesn't sleep.
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    filetime::set_file_mtime(&source, filetime::FileTime::from_system_time(earlier)).ok();

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();

    let target = match outcome {
        Outcome::Imported { target, .. } => target,
        other => panic!("expected Imported, got {other:?}"),
    };

    // No RAR writer exists, so a matched CBR is re-emitted as a CBZ at
    // the canonical `.cbz` path — the conversion accepted in kickoff Q3.
    assert_eq!(
        target,
        f.library
            .path()
            .join("Saga (2012)")
            .join("Saga (2012) 001.cbz")
    );
    assert!(target.exists());
    assert!(!source.exists(), "CBR source removed after import");

    // The output is a valid ZIP: the page survived the RAR→ZIP
    // conversion and ComicInfo was regenerated from the catalog.
    let entries = list_cbz_entries(&target);
    assert!(
        entries.iter().any(|e| e == "page-001.jpg"),
        "page lost in conversion: {entries:?}"
    );
    let xml = read_cbz_entry(&target, "ComicInfo.xml").expect("ComicInfo.xml missing");
    assert!(xml.contains("<Series>Saga</Series>"));
    assert!(xml.contains("<Web>https://comicvine.gamespot.com/issue/4000-364354/</Web>"));
}

#[tokio::test]
async fn overwrites_existing_comicinfo_in_source() {
    let f = seed_basic_fixture().await;

    let stale_xml = r#"<?xml version="1.0"?><ComicInfo><Series>Stale Data</Series><Number>999</Number></ComicInfo>"#;
    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, Some(stale_xml));

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
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
    assert_eq!(
        count, 1,
        "exactly one ComicInfo.xml expected, got {entries:?}"
    );
}

#[tokio::test]
async fn sweep_now_honors_live_match_confidence_threshold_from_settings() {
    // Finding 1 regression: Phase B used to apply the compiled
    // PHASE_B_OWNED_THRESHOLD constant (0.85), ignoring the DB
    // settings row that the scanner reads. Now `sweep_now` loads
    // `match_confidence_threshold` from settings before draining the
    // watch folder, so a value the scanner would accept also lands as
    // owned in Phase B.
    //
    // Trip the load-bearing case directly: set the row to a low value
    // (0.10) so even the weak filename-only path classifies as owned,
    // and confirm the file imports rather than skipping.
    let f = seed_basic_fixture().await;
    longbox_db::settings_repo::set(&f.db, "min_file_size_mb", "0")
        .await
        .unwrap();
    longbox_db::settings_repo::set(&f.db, "match_confidence_threshold", "0.10")
        .await
        .unwrap();

    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let cache = Arc::new(longbox_postprocess::PendingInterventionsCache::new());
    let summary = longbox_postprocess::sweep_now(
        f.library.path(),
        f.library_root_id,
        f._watch.path(),
        f.db.clone(),
        Arc::clone(&cache),
    )
    .await
    .unwrap();

    assert_eq!(
        summary.processed, 1,
        "DB threshold (0.10) must let the filename-match through; \
         summary={summary:?}"
    );
    assert!(!source.exists(), "matched source must have moved out of /watch/");
}

#[tokio::test]
async fn rejects_files_below_min_size_threshold_and_leaves_them_in_watch() {
    // Phase B size floor: a CBR/CBZ smaller than `min_file_size_mb`
    // bypasses every downstream stage (archive open, ComicInfo parse,
    // candidate lookup) and lands as `Outcome::RejectedTooSmall`.
    // The file STAYS in /watch/ — same disposition as Skipped — so
    // the operator can decide whether to replace the bad download or
    // delete it. The stub CBZ here is well under 35 MB (~200 bytes),
    // so a 35 MB threshold trips the floor unambiguously.
    let f = seed_basic_fixture().await;
    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        35,
    )
    .await
    .unwrap();

    match outcome {
        Outcome::RejectedTooSmall { size, threshold_mb } => {
            assert!(
                size < 35 * 1024 * 1024,
                "fixture must be under the threshold; got {size} bytes"
            );
            assert_eq!(threshold_mb, 35);
        }
        other => panic!("expected RejectedTooSmall, got {other:?}"),
    }

    assert!(
        source.exists(),
        "rejected source must stay in /watch/ for operator review"
    );
    let row = longbox_db::file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "Saga (2012)/Saga (2012) 001.cbz",
    )
    .await
    .unwrap();
    assert!(
        row.is_none(),
        "rejected files must not create catalog rows"
    );
}

#[tokio::test]
async fn min_size_zero_disables_the_floor_for_test_fixtures() {
    // The 0 sentinel is what every other test passes — explicit
    // confirmation it's a no-op so future test authors don't get
    // surprised by an unexpected RejectedTooSmall on their 200-byte
    // stubs. With `min_file_size_mb = 0`, the size check
    // (`size < 0 * 1024 * 1024`) is never satisfied; the file flows
    // through to the normal import path.
    let f = seed_basic_fixture().await;
    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, Outcome::Imported { .. }),
        "min_size=0 must not reject; got {outcome:?}"
    );
}

#[tokio::test]
async fn unmatched_file_stays_in_watch_folder() {
    // Per Jeremy's directive: the `_unsorted/` parking lot is gone.
    // When Phase B can't match a downloaded file to a catalog series,
    // it leaves the file in /watch/ untouched, emits a
    // `phase_b.skipped` WARN with the reason, and writes NO catalog
    // row. The watch folder is the holding pen.
    let f = seed_basic_fixture().await;

    let source = f._watch.path().join("Totally Unknown 042.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();
    let reason = match outcome {
        Outcome::Skipped { reason } => reason,
        other => panic!("expected Skipped, got {other:?}"),
    };
    assert!(
        reason.contains("Totally Unknown") || reason.contains("no catalog match"),
        "WARN reason should be operator-readable; got {reason:?}"
    );

    // Source untouched: still sitting at the watch-folder path.
    assert!(
        source.exists(),
        "Skipped file must stay in /watch/ — no _unsorted/ migration"
    );

    // No `_unsorted/` directory was ever created under library_root.
    assert!(
        !f.library.path().join("_unsorted").exists(),
        "library root must not gain a phantom _unsorted/ directory"
    );

    // No catalog row written — the file isn't in the library, so it
    // can't be in `files`.
    let row = longbox_db::file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "_unsorted/Totally Unknown 042.cbz",
    )
    .await
    .unwrap();
    assert!(row.is_none(), "no DB row should exist for an unmatched watch-folder file");
}

#[tokio::test]
async fn sub_threshold_match_imports_when_pull_attempt_exists() {
    // Trust override: when Phase B's local matcher scores below the
    // owned threshold, but the pull engine had previously asked for
    // this issue (any pull_attempts row, any status), the file is
    // accepted as Owned. The pull engine's indexer-time series-title
    // filter already cleared the title; the local floor would be
    // double-jeopardy. See live-log examples in the bug report:
    // "Y The Last Man" at 0.71, "Hell to Pay" at 0.73, "DC K.O. ..."
    // at 0.67 — all real series, all sub-threshold under 0.75, all
    // originally requested by the pull engine.
    let f = seed_basic_fixture().await;

    // File whose filename parses to "Saga" issue 1 — same series as
    // the seeded catalog row. Force a sub-threshold local score by
    // passing an impossibly strict threshold (0.99). Without the
    // pull_attempt this should land as Skipped (NeedsReview tier);
    // with the pull_attempt it must Import.
    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    longbox_db::pull_attempt_repo::insert(
        &f.db,
        longbox_db::NewPullAttempt {
            series_id: f.series_id,
            issue_id: f.issue_id,
            indexer_id: None,
            release_id: Some("guid-saga-001-pulled".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-saga-001".into()),
        },
    )
    .await
    .unwrap();

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
        0.99, // tighter than any real local score → forces NeedsReview
        0,
    )
    .await
    .unwrap();

    assert!(
        matches!(
            outcome,
            Outcome::Imported {
                series_id,
                issue_id,
                ..
            } if series_id == f.series_id && issue_id == f.issue_id
        ),
        "sub-threshold match must import when a pull_attempt exists; got {outcome:?}"
    );
}

#[tokio::test]
async fn sub_threshold_match_skipped_when_no_pull_attempt_exists() {
    // Counterpart: the trust override is gated on pull-attempt
    // history. A sub-threshold match for an issue we did NOT ask
    // for stays Skipped — the file might be the right series at low
    // confidence, but it might also be wrong-volume drift. The
    // existing 0.75-floor behavior is preserved for organic
    // arrivals.
    let f = seed_basic_fixture().await;
    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    // No pull_attempt row inserted.

    let outcome = processor::process_one(
        &source,
        f.library.path(),
        f.library_root_id,
        &f.db,
        0.99,
        0,
    )
    .await
    .unwrap();

    assert!(
        matches!(outcome, Outcome::Skipped { .. }),
        "sub-threshold match without pull_attempt history must stay Skipped; got {outcome:?}"
    );
    assert!(
        source.exists(),
        "Skipped file must stay in /watch/"
    );
}

#[tokio::test]
async fn conflict_cleans_source_and_preserves_target() {
    // Phase B detects target_abs.exists() → Conflict. The library
    // already owns canonical bytes for this (series, issue), so
    // process_one removes the duplicate from the watch folder
    // instead of stranding it forever (the prior behavior was a
    // documented "leaves files stranded" complaint). Target bytes
    // and absence of a catalog row are the load-bearing invariants
    // — the cleanup is best-effort but expected.
    let f = seed_basic_fixture().await;

    let target_dir = f.library.path().join("Saga (2012)");
    std::fs::create_dir_all(&target_dir).unwrap();
    let existing_target = target_dir.join("Saga (2012) 001.cbz");
    std::fs::write(&existing_target, b"pre-existing").unwrap();

    let source = f._watch.path().join("Saga 001.cbz");
    write_cbz(&source, None);

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();
    match outcome {
        Outcome::Conflict { target, .. } => assert_eq!(target, existing_target),
        other => panic!("expected Conflict, got {other:?}"),
    }

    assert!(
        !source.exists(),
        "source must be cleaned up on conflict — leaving it strands the duplicate"
    );
    let existing_bytes = std::fs::read(&existing_target).unwrap();
    assert_eq!(
        existing_bytes, b"pre-existing",
        "target must not be overwritten"
    );
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
async fn conflict_cleanup_skips_when_source_is_target() {
    // Re-processing a file that already sits at its canonical library
    // path: source == target, so the cleanup MUST short-circuit or it
    // would delete the library's canonical bytes. This guards the
    // `idempotent_reprocessing` invariant from regressing if the
    // cleanup logic ever changes.
    let f = seed_basic_fixture().await;
    let target_dir = f.library.path().join("Saga (2012)");
    std::fs::create_dir_all(&target_dir).unwrap();
    let at_target = target_dir.join("Saga (2012) 001.cbz");
    write_cbz(&at_target, None);
    let bytes_before = std::fs::read(&at_target).unwrap();

    let outcome = processor::process_one(&at_target, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();
    assert!(matches!(outcome, Outcome::Conflict { .. }));
    assert!(
        at_target.exists(),
        "file at canonical path must survive same-file conflict cleanup"
    );
    assert_eq!(
        std::fs::read(&at_target).unwrap(),
        bytes_before,
        "bytes must be untouched"
    );
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
    let first = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();
    let target = match first {
        Outcome::Imported { target, .. } => target,
        _ => panic!(),
    };

    // Reprocess the target itself: it already lives in the library
    // and has a catalog row. process_one should detect the conflict
    // (target exists at its own path).
    let outcome2 = processor::process_one(&target, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
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

    let outcome = processor::process_one(&source, f.library.path(), f.library_root_id, &f.db, longbox_core::DEFAULT_MATCH_THRESHOLD, 0)
        .await
        .unwrap();

    match outcome {
        Outcome::Failed {
            reason,
            size,
            target,
        } => {
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
