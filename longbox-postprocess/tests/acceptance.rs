//! Phase B Step 8 acceptance smoke. Drives the real public entry point
//! `longbox_postprocess::start()` against a tempdir with six source
//! files of varying provenance, then asserts every one of the brief's
//! seven "done" criteria in a single test pass.
//!
//! File composition (file label → criterion validated):
//! - **a**: ComicInfo + known series, normal arrival → criteria 1, 2, 3, 6
//!   (detect, identify, owned import, `matched_at` populated for the
//!   Phase A.6 activity feed)
//! - **b**: filename-only fallback against a known series → criterion 3
//!   alternate branch (no ComicInfo, matcher leans on filename)
//! - **c**: ComicInfo for a series not in the catalog → criterion 4
//!   (Skipped, stays in /watch/ per Jeremy's directive — no
//!   `_unsorted/` parking lot, no catalog row)
//! - **d**: no ComicInfo, filename with no recognizable shape →
//!   criterion 4 alt-branch (the no-hint short-circuit; also Skipped)
//! - **e**: matches a known issue whose target path already exists →
//!   criterion 5 (conflict; source auto-removed by
//!   `cleanup_conflict_source`; cache stays empty)
//! - **f**: present in the watch folder BEFORE `start()` is called →
//!   criterion 7 (initial-sweep picks up pre-existing files at boot)
//!
//! Criterion 6 (`matched_at` feeds the "Recently completed issues"
//! activity feed) is checked via the row state for a, b, f after import.
//! Criterion 7's "container restart while mid-process" partial-state
//! recovery is out of scope here — brief's guarantee for that case is
//! tempfile-Drop cleanup, which SIGKILL bypasses; not testable without
//! killing tokio runtimes mid-flight.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use longbox_db::{
    file_repo, issue_repo, library_root_repo, series_repo, NewIssue, NewLibraryRoot, NewSeries,
    Pool,
};
use longbox_postprocess::{PendingInterventionsCache, PostprocessConfig};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

struct AcceptanceFixture {
    watch: TempDir,
    library: TempDir,
    db: Pool,
    library_root_id: i64,
    saga_1_id: i64,
    saga_2_id: i64,
    hellboy_1_id: i64,
    watchmen_1_id: i64,
}

async fn seed_acceptance_fixture() -> AcceptanceFixture {
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

    let saga_id = insert_series(&db, "Saga", "saga", 2012, "Image").await;
    let hellboy_id = insert_series(&db, "Hellboy", "hellboy", 1994, "Dark Horse").await;
    let watchmen_id = insert_series(&db, "Watchmen", "watchmen", 1986, "DC").await;

    let saga_1_id = insert_issue(&db, saga_id, "1", "2012-03-14").await;
    let saga_2_id = insert_issue(&db, saga_id, "2", "2012-04-18").await;
    let hellboy_1_id = insert_issue(&db, hellboy_id, "1", "1994-03-01").await;
    let watchmen_1_id = insert_issue(&db, watchmen_id, "1", "1986-09-01").await;

    AcceptanceFixture {
        watch,
        library,
        db,
        library_root_id,
        saga_1_id,
        saga_2_id,
        hellboy_1_id,
        watchmen_1_id,
    }
}

async fn insert_series(db: &Pool, title: &str, sort: &str, year: i32, pub_: &str) -> i64 {
    series_repo::insert(
        db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: sort.into(),
            start_year: Some(year),
            publisher: Some(pub_.into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

async fn insert_issue(db: &Pool, series_id: i64, number: &str, cover_date: &str) -> i64 {
    issue_repo::insert(
        db,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.into(),
            title: None,
            cover_date: Some(cover_date.into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

/// Write a CBZ with optional ComicInfo, then push mtime 10 s into the
/// past so the 2 s stability check doesn't sleep. Same pattern as
/// `pipeline.rs` — we're measuring end-to-end orchestration, not the
/// stability wait.
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
    let earlier = std::time::SystemTime::now() - Duration::from_secs(10);
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(earlier)).ok();
}

fn comic_info_xml(series: &str, number: &str, year: i32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Series>{series}</Series>
  <Number>{number}</Number>
  <Year>{year}</Year>
</ComicInfo>"#
    )
}

/// Poll `cond` every 100 ms until it returns true or `timeout` elapses.
/// Used to wait for the async pipeline to settle after dropping files.
/// Panics on timeout with the supplied label so the failure message
/// names what the test was waiting for.
async fn wait_until<F, Fut>(timeout: Duration, label: &str, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    loop {
        if cond().await {
            return;
        }
        if start.elapsed() > timeout {
            panic!("acceptance test timed out waiting for: {label} (after {timeout:?})");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn six_file_acceptance_smoke() {
    let f = seed_acceptance_fixture().await;

    // Stub CBZs in this test are ~200 bytes; the seeded
    // `min_file_size_mb=10` would reject every one of them. Override
    // the setting to 0 (no floor) before the watcher starts so the
    // test exercises the normal import path. Production deployments
    // keep the seeded 10 — see `tests/pipeline.rs` for the dedicated
    // RejectedTooSmall regression.
    longbox_db::settings_repo::set(&f.db, "min_file_size_mb", "0")
        .await
        .unwrap();

    // --- Pre-stage state that must exist BEFORE start() ---

    // For file (e)'s conflict: a real file already sitting at the
    // library path Hellboy #1 will compute to. process_one's
    // `target_abs.exists()` check needs a true filesystem-side hit.
    let conflict_target = f
        .library
        .path()
        .join("Hellboy (1994)")
        .join("Hellboy (1994) 001.cbz");
    std::fs::create_dir_all(conflict_target.parent().unwrap()).unwrap();
    std::fs::write(&conflict_target, b"pre-existing hellboy bytes").unwrap();
    let conflict_target_bytes_before = std::fs::read(&conflict_target).unwrap();

    // For file (f): a CBZ in the watch folder BEFORE the pipeline
    // starts. `initial_sweep` is supposed to pick this up at boot — the
    // criterion-7 recovery path.
    let f_path = f.watch.path().join("Watchmen 001.cbz");
    write_cbz(&f_path, None);

    // --- Start the real pipeline ---

    let cache = Arc::new(PendingInterventionsCache::new());
    let config = PostprocessConfig {
        watch_path: f.watch.path().to_path_buf(),
        library_root: f.library.path().to_path_buf(),
        // Tight poll for the acceptance test — the test post-drops
        // files and waits ~5s for them to land. Default 30s would
        // make the test wall-time absurd.
        poll_interval: Duration::from_millis(500),
    };
    longbox_postprocess::start(config, f.db.clone(), Arc::clone(&cache))
        .await
        .unwrap();

    // notify can drop events that arrive synchronously with watcher
    // setup on some platforms (matches the grace period in
    // `live_detection.rs`). Without this, files dropped immediately
    // after `start()` returns may be missed.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // --- Drop the remaining five files AFTER start(). The notify
    //     watcher picks each one up. Order is irrelevant; the pipeline
    //     is serial per the brief.

    // (a) ComicInfo + known series → owned import
    let a_path = f.watch.path().join("Saga 001.cbz");
    write_cbz(&a_path, Some(&comic_info_xml("Saga", "1", 2012)));

    // (b) filename-only against a known series → owned import
    let b_path = f.watch.path().join("Saga 002.cbz");
    write_cbz(&b_path, None);

    // (c) ComicInfo for a series NOT in the catalog → Skipped, stays
    //     in /watch/. Per Jeremy's directive: no _unsorted/ parking
    //     lot. The watch folder IS the holding pen.
    let c_path = f.watch.path().join("Marvel 1602 001.cbz");
    write_cbz(&c_path, Some(&comic_info_xml("Marvel 1602", "1", 2003)));

    // (d) no ComicInfo, filename with no parseable issue → Skipped via
    //     the no-hint short-circuit. The filename has no digit and no
    //     recognizable {series} {number} shape. Stays in /watch/.
    let d_path = f.watch.path().join("garbage_no_number.cbz");
    write_cbz(&d_path, None);

    // (e) matches Hellboy #1 → target path exists → conflict
    let e_path = f.watch.path().join("Hellboy 001.cbz");
    write_cbz(&e_path, None);

    // --- Poll for the pipeline to settle ---
    //
    // Terminal state: 3 catalog rows present (a, b, f — the three
    // owned imports); files (c) and (d) stay in /watch/ as Skipped
    // (no _unsorted/ — Jeremy's directive); file (e)'s conflict is
    // auto-cleaned (source removed from watch); cache is empty.
    // 30 s outer timeout is generous; settles well under a second.
    let library_root_id = f.library_root_id;
    let db = f.db.clone();
    let cache_for_poll = Arc::clone(&cache);
    let c_for_poll = c_path.clone();
    let d_for_poll = d_path.clone();
    wait_until(Duration::from_secs(30), "pipeline to settle", || {
        let db = db.clone();
        let cache = Arc::clone(&cache_for_poll);
        let c = c_for_poll.clone();
        let d = d_for_poll.clone();
        async move {
            let saga_001 =
                file_repo::find_by_path(&db, library_root_id, "Saga (2012)/Saga (2012) 001.cbz")
                    .await
                    .unwrap();
            let saga_002 =
                file_repo::find_by_path(&db, library_root_id, "Saga (2012)/Saga (2012) 002.cbz")
                    .await
                    .unwrap();
            let watchmen_001 = file_repo::find_by_path(
                &db,
                library_root_id,
                "Watchmen (1986)/Watchmen (1986) 001.cbz",
            )
            .await
            .unwrap();
            // Skipped files must still be sitting at their source path
            // in /watch/. We poll on filesystem presence rather than DB
            // rows because Skipped intentionally writes no catalog row.
            saga_001.is_some()
                && saga_002.is_some()
                && watchmen_001.is_some()
                && c.exists()
                && d.exists()
                && cache.is_empty()
        }
    })
    .await;

    // --- Assertions, criterion-by-criterion ---

    // Criterion 1, 2, 3, 6 — file (a): detect, identify, owned import,
    // matched_at populated.
    let row_a =
        file_repo::find_by_path(&f.db, f.library_root_id, "Saga (2012)/Saga (2012) 001.cbz")
            .await
            .unwrap()
            .expect("file a must be catalogued as owned");
    assert_eq!(row_a.issue_id, Some(f.saga_1_id), "file a → Saga #1");
    assert_eq!(row_a.status, "owned");
    assert_eq!(row_a.match_method, "phase_b");
    assert!(
        (row_a.match_confidence - 1.0).abs() < f64::EPSILON,
        "phase_b owned confidence is 1.0, got {}",
        row_a.match_confidence
    );
    assert!(
        row_a.matched_at.is_some(),
        "matched_at must be populated for activity feed (criterion 6)"
    );
    assert!(!a_path.exists(), "source (a) must have moved out of watch");

    // Imported CBZ carries both ComicInfo.xml and MetronInfo.xml at the
    // archive root. The names must be exactly those — no subdirectory
    // prefix — because every consumer (Perdoo, ComicRack CE, Codex,
    // Comicbox, Komga, Kavita) reads them from root only.
    let imported_cbz = f
        .library
        .path()
        .join("Saga (2012)")
        .join("Saga (2012) 001.cbz");
    let cbz_bytes = std::fs::read(&imported_cbz).expect("imported CBZ must exist");
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(cbz_bytes)).expect("imported CBZ must be a valid ZIP");
    let entry_names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_owned())
        .collect();
    assert!(
        entry_names.iter().any(|n| n == "ComicInfo.xml"),
        "ComicInfo.xml at root (regression): entries = {entry_names:?}"
    );
    assert!(
        entry_names.iter().any(|n| n == "MetronInfo.xml"),
        "MetronInfo.xml at root: entries = {entry_names:?}"
    );
    // Explicitly assert no subdirectory-nested copy snuck in — the
    // load-bearing "at root, not in a subdirectory" guarantee.
    assert!(
        !entry_names
            .iter()
            .any(|n| n.contains('/') && n.ends_with("MetronInfo.xml")),
        "MetronInfo.xml must only live at root, never under a subdir: entries = {entry_names:?}"
    );

    // Read MetronInfo.xml and verify the core fields land. We're not
    // re-testing the writer's full output (the writer's own golden
    // tests do that); this checks the wiring from
    // (SeriesRow, IssueRow) → on-disk archive really fires.
    let mut metron_entry = archive
        .by_name("MetronInfo.xml")
        .expect("MetronInfo.xml openable by name");
    let mut metron_xml = String::new();
    std::io::Read::read_to_string(&mut metron_entry, &mut metron_xml)
        .expect("MetronInfo.xml is UTF-8");
    drop(metron_entry);
    assert!(
        metron_xml.contains("<MetronInfo"),
        "root element present: {metron_xml}"
    );
    assert!(
        metron_xml.contains("<Name>Saga</Name>"),
        "series name landed: {metron_xml}"
    );
    assert!(
        metron_xml.contains("<Number>1</Number>"),
        "issue number landed: {metron_xml}"
    );
    assert!(
        metron_xml.contains("<StartYear>2012</StartYear>"),
        "series start year landed: {metron_xml}"
    );
    assert!(
        metron_xml.contains("<LastModified>"),
        "LastModified timestamp present: {metron_xml}"
    );

    // Criterion 3 alt — file (b): filename-only match.
    let row_b =
        file_repo::find_by_path(&f.db, f.library_root_id, "Saga (2012)/Saga (2012) 002.cbz")
            .await
            .unwrap()
            .expect("file b must be catalogued as owned");
    assert_eq!(row_b.issue_id, Some(f.saga_2_id), "file b → Saga #2");
    assert_eq!(row_b.status, "owned");
    assert_eq!(row_b.match_method, "phase_b");
    assert!(row_b.matched_at.is_some());

    // Criterion 4 — file (c): ComicInfo for unknown series → Skipped,
    // file STAYS in /watch/. Per Jeremy's directive: no _unsorted/
    // parking lot. The WARN log carries the reason; the operator can
    // see the file directly in the watch folder.
    assert!(
        c_path.exists(),
        "source (c) must stay in /watch/ — no _unsorted/ migration"
    );
    let row_c = file_repo::find_by_path(&f.db, f.library_root_id, "_unsorted/Marvel 1602 001.cbz")
        .await
        .unwrap();
    assert!(
        row_c.is_none(),
        "no catalog row should exist for the Skipped file (c)"
    );

    // Criterion 4 alt — file (d): no-hint short-circuit → Skipped,
    // file STAYS in /watch/.
    assert!(
        d_path.exists(),
        "source (d) must stay in /watch/ — no _unsorted/ migration"
    );
    let row_d =
        file_repo::find_by_path(&f.db, f.library_root_id, "_unsorted/garbage_no_number.cbz")
            .await
            .unwrap();
    assert!(
        row_d.is_none(),
        "no catalog row should exist for the Skipped file (d)"
    );

    // Library root must never gain a phantom `_unsorted/` directory —
    // the move_to_unsorted path is entirely removed.
    assert!(
        !f.library.path().join("_unsorted").exists(),
        "library root must not contain an _unsorted/ directory anymore"
    );

    // Criterion 5 — file (e): target exists → conflict; source is
    // auto-removed from the watch folder (the library already owns
    // canonical bytes, leaving the dupe pending forever clutters the
    // complete folder); target bytes preserved; cache stays empty
    // (cleaned-up conflicts are no longer pending interventions); no
    // catalog row written.
    assert!(
        !e_path.exists(),
        "source (e) must be cleaned up on conflict, not stranded in watch"
    );
    let bytes_after = std::fs::read(&conflict_target).unwrap();
    assert_eq!(
        bytes_after, conflict_target_bytes_before,
        "pre-existing target bytes must not have been overwritten"
    );
    let e_row = file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "Hellboy (1994)/Hellboy (1994) 001.cbz",
    )
    .await
    .unwrap();
    assert!(
        e_row.is_none(),
        "no catalog row should be written for the conflicting source"
    );
    let snap = cache.snapshot();
    assert!(
        snap.iter().all(|item| {
            std::fs::canonicalize(&item.source_path)
                .ok()
                .zip(std::fs::canonicalize(&e_path).ok())
                .map(|(a, b)| a != b)
                .unwrap_or(true)
        }),
        "conflict (e) must NOT appear as a pending intervention — \
         the source is cleaned up so there's nothing to intervene on"
    );

    // Criterion 7 — file (f): pre-staged before start(), picked up by
    // initial_sweep, ends up as owned just like a post-start arrival.
    let row_f = file_repo::find_by_path(
        &f.db,
        f.library_root_id,
        "Watchmen (1986)/Watchmen (1986) 001.cbz",
    )
    .await
    .unwrap()
    .expect("file f (pre-staged) must be catalogued via initial-sweep");
    assert_eq!(row_f.issue_id, Some(f.watchmen_1_id));
    assert_eq!(row_f.status, "owned");
    assert_eq!(row_f.match_method, "phase_b");
    assert!(
        row_f.matched_at.is_some(),
        "initial-sweep imports must populate matched_at (criterion 6 + 7)"
    );
    assert!(
        !f_path.exists(),
        "source (f) must have moved out of watch after initial-sweep"
    );

    // Activity-feed sanity: exactly three rows have matched_at set,
    // matching the three owned imports. The two unmatched rows leave
    // matched_at NULL (per next_matched_at policy for issue_id=None).
    let all_rows = file_repo::list_by_library_root(&f.db, f.library_root_id)
        .await
        .unwrap();
    let matched_count = all_rows.iter().filter(|r| r.matched_at.is_some()).count();
    assert_eq!(
        matched_count, 3,
        "activity feed (criterion 6) should see exactly the 3 owned imports"
    );

    // Suppress unused warnings on the issue id we don't directly
    // assert on (e was not imported, so hellboy_1_id is contextual).
    let _ = f.hellboy_1_id;
}
