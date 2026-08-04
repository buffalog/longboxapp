//! Phase A.8 Step 6 — Phase B pull-engine attribution.
//!
//! When Phase B catches a file whose (series, issue) has an in-flight
//! `pull_attempt`, the file is catalogued `match_method='pull_list'`
//! and the attempt(s) settle to `grabbed`. With no attempt it is an
//! ordinary `phase_b` catch.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime};

use longbox_db::{
    file_repo, issue_repo, library_root_repo, pull_attempt_repo, series_repo, NewIssue,
    NewLibraryRoot, NewPullAttempt, NewSeries, Pool,
};
use longbox_postprocess::processor::{process_one, Outcome};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

struct Fixture {
    _library: TempDir,
    watch: TempDir,
    db: Pool,
    library_root_id: i64,
    library_root: std::path::PathBuf,
    series_id: i64,
    issue_id: i64,
}

async fn seed() -> Fixture {
    let db = longbox_db::open(":memory:").await.unwrap();
    let library = TempDir::new().unwrap();
    let watch = TempDir::new().unwrap();
    let library_root = library.path().to_path_buf();

    let library_root_id = library_root_repo::insert(
        &db,
        NewLibraryRoot {
            path: library_root.to_string_lossy().into_owned(),
        },
    )
    .await
    .unwrap()
    .id;
    let series_id = series_repo::insert(
        &db,
        NewSeries {
            cv_id: None,
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
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: Some("2012-03-14".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    Fixture {
        _library: library,
        watch,
        db,
        library_root_id,
        library_root,
        series_id,
        issue_id,
    }
}

/// Write a Saga #1 CBZ with a matching ComicInfo, mtime backdated so
/// `process_one`'s stability wait is a no-op.
fn write_saga_1(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("page-001.jpg", opts).unwrap();
    zip.write_all(b"\xFF\xD8\xFF\xE0\x00\x10JFIF").unwrap();
    zip.start_file("ComicInfo.xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?><ComicInfo><Series>Saga</Series><Number>1</Number><Year>2012</Year></ComicInfo>"#,
    )
    .unwrap();
    zip.finish().unwrap();
    let earlier = SystemTime::now() - Duration::from_secs(10);
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(earlier)).ok();
}

async fn seed_in_flight_attempt(db: &Pool, series_id: i64, issue_id: i64) {
    pull_attempt_repo::insert(
        db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-abc".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-123".into()),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn caught_file_with_an_in_flight_attempt_is_attributed_to_the_pull_engine() {
    let f = seed().await;
    seed_in_flight_attempt(&f.db, f.series_id, f.issue_id).await;

    let source = f.watch.path().join("Saga 001.cbz");
    write_saga_1(&source);

    let outcome = process_one(
        &source,
        &f.library_root,
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await
    .unwrap();
    let Outcome::Imported {
        file_id, issue_id, ..
    } = outcome
    else {
        panic!("expected Imported, got {outcome:?}");
    };
    assert_eq!(issue_id, f.issue_id);

    // The catalogued file is tagged pull_list, not phase_b.
    let file = file_repo::find_by_id(&f.db, file_id)
        .await
        .unwrap()
        .expect("imported file row");
    assert_eq!(file.match_method, "pull_list");

    // The in-flight attempt has settled to grabbed.
    let attempts = pull_attempt_repo::list_for_issue(&f.db, f.series_id, f.issue_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].status, "grabbed");
}

#[tokio::test]
async fn caught_file_without_an_attempt_is_an_ordinary_phase_b_catch() {
    let f = seed().await;
    // No pull_attempt seeded — this is a manual SAB drop, not a pull.

    let source = f.watch.path().join("Saga 001.cbz");
    write_saga_1(&source);

    let outcome = process_one(
        &source,
        &f.library_root,
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await
    .unwrap();
    let Outcome::Imported { file_id, .. } = outcome else {
        panic!("expected Imported, got {outcome:?}");
    };
    let file = file_repo::find_by_id(&f.db, file_id)
        .await
        .unwrap()
        .expect("imported file row");
    assert_eq!(file.match_method, "phase_b");
}

// ---- corrupt-download attribution ----------------------------------
//
// SAB cannot detect this class. Observed live: 47 comic jobs in one
// SAB log, every one "No par2 sets for <job>" and zero repair events —
// these NZBs carry no recovery set, so SAB has no checksums, reports
// Completed in good faith, and the CRC error inside the RAR is only
// discovered when Phase B opens the archive.
//
// Phase B is therefore the only component positioned to detect it, and
// it used to throw the knowledge away: it logged `phase_b.failed`, left
// the file in the watch folder, and left the attempt `submitted` to age
// out three sweeps later as "lost track of download" — a reason that is
// simply false. We did not lose track. We know exactly what happened.

/// Write the real corrupt-download shape: a RAR whose headers and
/// `ComicInfo.xml` read cleanly, but whose page data fails its CRC.
///
/// This is the shape that actually reaches the rewrite. A file that is
/// not an archive at all fails earlier, in `read_comic_info_xml`, and
/// never gets far enough to be attributed to an issue — so a fixture
/// built that way would test a different code path than the one the
/// three live corrupt downloads exercised.
///
/// Byte 72 of the vendored fixture sits inside `page-001.jpg`'s
/// compressed data; flipping it leaves the entry table and the
/// ComicInfo entry intact. Verified with `unrar t`: exactly
/// `page-001.jpg - checksum error`.
fn write_crc_corrupt_archive(path: &Path) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-rar5.cbr");
    let mut bytes = std::fs::read(fixture).unwrap();
    bytes[72] ^= 0xFF;
    std::fs::write(path, &bytes).unwrap();
    let earlier = SystemTime::now() - Duration::from_secs(10);
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(earlier)).ok();
}

async fn attempt_status(db: &Pool, series_id: i64, issue_id: i64) -> (String, Option<String>) {
    let rows = pull_attempt_repo::list_for_issue(db, series_id, issue_id)
        .await
        .unwrap();
    let a = rows.first().expect("an attempt row");
    (a.status.clone(), a.error_message.clone())
}

#[tokio::test]
async fn an_unreadable_download_records_the_real_reason_on_the_attempt() {
    let f = seed().await;
    seed_in_flight_attempt(&f.db, f.series_id, f.issue_id).await;

    let source = f.watch.path().join("Saga 001.cbr");
    write_crc_corrupt_archive(&source);

    let outcome = process_one(
        &source,
        &f.library_root,
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await
    .unwrap();

    let (status, message) = attempt_status(&f.db, f.series_id, f.issue_id).await;
    assert_eq!(
        status, "failed",
        "the attempt must settle now, not age out as 'lost track of download': {outcome:?}"
    );
    let message = message.expect("a reason must be recorded");
    assert!(
        message.to_lowercase().contains("unreadable"),
        "the reason must name what actually happened, got {message:?}"
    );
}

/// The guard that makes the above safe: a failure to WRITE the output
/// is a local problem — a full disk, a read-only library — and says
/// nothing about the release. Blaming the release for it would fail a
/// perfectly good download and burn a retry on every one of them.
///
/// This test is vacuous before the fix (nothing records failures at
/// all); its value is as the regression bar afterwards, and it is
/// mutation-verified by mis-classifying the two failure sides.
#[tokio::test]
async fn a_local_write_failure_does_not_blame_the_release() {
    let f = seed().await;
    seed_in_flight_attempt(&f.db, f.series_id, f.issue_id).await;

    let source = f.watch.path().join("Saga 001.cbz");
    write_saga_1(&source); // a perfectly good archive

    // Make the library root unwritable so the temp CBZ cannot be created.
    let mut perms = std::fs::metadata(&f.library_root).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
    }
    std::fs::set_permissions(&f.library_root, perms).unwrap();

    let outcome = process_one(
        &source,
        &f.library_root,
        f.library_root_id,
        &f.db,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
        0,
    )
    .await;

    // Restore before asserting so a failure cannot leave an undeletable tempdir.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&f.library_root).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&f.library_root, p).unwrap();
    }
    let outcome = outcome.unwrap();

    let (status, _) = attempt_status(&f.db, f.series_id, f.issue_id).await;
    assert_eq!(
        status, "submitted",
        "a local write failure must not fail the attempt: {outcome:?}"
    );
}
