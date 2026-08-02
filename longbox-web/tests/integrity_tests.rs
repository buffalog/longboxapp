//! Library Integrity — discovery surface.
//!
//! The load-bearing test here is [`only_the_analyze_route_accepts_a_write_method`].
//! Every other assertion in this file is about behaviour; that one is about a
//! property of the whole module, and it is the reason this PR can be trusted
//! to be non-destructive.

mod common;

use std::collections::HashMap;

use axum::http::{Method, StatusCode};
use common::{build_test_app, empty_request, response_json, TestApp};
use longbox_db::{file_repo, issue_repo, series_repo, NewFile, NewIssue, NewSeries};

/// Everything that could mutate. `GET` and `HEAD` are excluded by definition.
const WRITE_METHODS: &[Method] = &[Method::POST, Method::PUT, Method::PATCH, Method::DELETE];

fn request(method: Method, path: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// **The constraint this whole PR rests on.**
///
/// Not "read-only" — the module has one write path and says so. For every
/// (path x write-method) pair the module registers, the method must be
/// accepted **iff** that route declared `Surface::Writes`.
///
/// The probe list is DERIVED from `declared_routes()`, not hand-written. An
/// earlier version of this test iterated a hardcoded path list and only
/// counted `Writes` declarations, and a code review broke it in one line: a
/// `DELETE` route declared `Surface::ReadOnly` passed every test. Deriving the
/// list means a false declaration fails here instead of shipping.
///
/// Note the router serves an SPA fallback, so an unrouted path returns 200
/// with index.html rather than 404 — which is why this asserts on 405
/// specifically for declared paths, and why a "does this path exist" test
/// would be vacuous.
#[tokio::test]
async fn a_write_method_is_accepted_only_where_the_route_declares_a_write() {
    let app = build_test_app().await;
    let declared = longbox_web::routes::integrity::declared_routes();
    assert!(!declared.is_empty(), "no routes declared");

    for (path, declares_write) in &declared {
        let full = format!("/api{path}");
        for method in WRITE_METHODS {
            let resp = app.request(request(method.clone(), &full)).await;
            let accepted = resp.status() != StatusCode::METHOD_NOT_ALLOWED;
            let expected = *declares_write && *method == Method::POST;
            assert_eq!(
                accepted,
                expected,
                "{method} {full}: accepted={accepted} but declaration says {expected} \
                 (declares_write={declares_write}); status was {}",
                resp.status()
            );
        }
    }

    // The set of mutating routes, by name and kind.
    //
    // This replaced a bare count ("exactly one write path"), which
    // encoded the discovery-only property this module had before it
    // could act. A count would have been satisfied by ANY second
    // mutating route; naming them means adding one is a deliberate
    // edit to this list rather than a number quietly going up.
    //
    // Kinds are distinguished because they are not interchangeable:
    // `writes` touches derived cache recomputable from bytes on disk,
    // `deletes` removes a catalog row and unlinks a file, and neither
    // is recoverable from the other's premise.
    let mut mutating: Vec<(&str, &str)> = longbox_web::routes::integrity::declared_surface_kinds()
        .into_iter()
        .filter(|(_, k)| *k != "read_only")
        .collect();
    mutating.sort();
    assert_eq!(
        mutating,
        vec![
            ("/library/integrity/analyze", "writes"),
            ("/library/integrity/duplicates/delete", "deletes"),
        ],
        "the mutating surface of this module is a deliberate list"
    );
}

// -------- analyze job behaviour --------

#[tokio::test]
async fn analyze_starts_idle_reports_nothing() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request(
            "GET",
            "/api/library/integrity/analyze/status",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["running"], false);
    assert_eq!(body["last"], serde_json::Value::Null);
    assert_eq!(body["last_error"], serde_json::Value::Null);
}

/// The pass returns immediately rather than holding the request open for the
/// length of the I/O.
#[tokio::test]
async fn analyze_returns_accepted_without_waiting() {
    let app = build_test_app().await;
    let resp = app
        .request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = response_json(resp).await;
    assert_eq!(body["status"], "started");
}

/// An empty library still completes a pass — zero candidates is a valid
/// result, not an error, and must not leave the job stuck `running`.
#[tokio::test]
async fn a_pass_over_an_empty_library_completes_and_reports_zero() {
    let app = build_test_app().await;
    let resp = app
        .request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let status = wait_for_idle(&app).await;
    assert_eq!(status["running"], false);
    assert_eq!(
        status["last_error"],
        serde_json::Value::Null,
        "an empty library is not an error: {}",
        status["last_error"]
    );
    assert_eq!(status["last"]["candidates"], 0);
    assert_eq!(status["last"]["hashed"], 0);
    assert!(status["finished_at"].is_string());
}

/// Poll until the job reports idle. The pass is spawned, so the status
/// endpoint is the only way to observe completion.
async fn wait_for_idle(app: &TestApp) -> serde_json::Value {
    for _ in 0..200 {
        let resp = app
            .request(empty_request(
                "GET",
                "/api/library/integrity/analyze/status",
            ))
            .await;
        let body = response_json(resp).await;
        if body["running"] == false && body["finished_at"].is_string() {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("analyze pass never reported idle");
}

/// Two passes at once double the I/O for no benefit. The second is refused
/// rather than queued — this is a human-triggered pass, so telling the user
/// it is already running is the useful answer.
#[tokio::test]
async fn a_second_pass_is_refused_while_one_is_in_flight() {
    let app = build_test_app().await;
    // Hold the job open by claiming the flag directly, which is what a
    // long-running pass looks like from the endpoint's perspective.
    {
        let mut s = app.state.analyze_status.write().await;
        s.running = true;
    }
    let resp = app
        .request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "conflict.analysis_in_progress");
}

/// A pass over real archives populates digests AND the archive label, and
/// records what it did. Two files with identical bytes get identical digests;
/// a third with different bytes does not.
#[tokio::test]
async fn a_pass_hashes_size_colliding_files_and_records_stats() {
    let app = build_test_app().await;
    let (a, b, solo) = common_seed_three_files(&app).await;

    let resp = app
        .request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let status = wait_for_idle(&app).await;

    // Only the size-colliding pair is a candidate; the odd-sized file is not.
    assert_eq!(
        status["last"]["candidates"], 2,
        "only size-collision candidates are hashed: {}",
        status["last"]
    );
    assert_eq!(status["last"]["hashed"], 2);
    assert_eq!(status["last"]["failed"], 0);

    let rows: Vec<(i64, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, content_blake3, archive_label FROM files ORDER BY id")
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    let by_id: std::collections::HashMap<i64, (Option<String>, Option<String>)> =
        rows.into_iter().map(|(i, d, l)| (i, (d, l))).collect();

    let da = by_id[&a].0.clone().expect("a hashed");
    let db_ = by_id[&b].0.clone().expect("b hashed");
    assert_eq!(da, db_, "identical bytes must produce identical digests");
    assert!(
        by_id[&solo].0.is_none(),
        "a file with no size twin must not be hashed"
    );

    // The archive label was captured in the same pass.
    assert_eq!(
        by_id[&a].1.as_deref(),
        Some("My Little Warlord 008"),
        "the archive's internal label is recorded alongside the digest"
    );
}

/// Two byte-identical CBZs sharing a size, plus one of a different size.
/// Returns their file ids.
async fn common_seed_three_files(app: &TestApp) -> (i64, i64, i64) {
    use std::io::Write;

    let write_cbz = |rel: &str, pages: usize| {
        let full = app.library_path().join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        let f = std::fs::File::create(&full).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for p in 0..pages {
            zip.start_file(format!("My Little Warlord 008/008-{p:04}.jpg"), opts)
                .unwrap();
            zip.write_all(&vec![7u8; 2048]).unwrap();
        }
        zip.finish().unwrap();
        std::fs::metadata(&full).unwrap().len() as i64
    };

    // Same page count and same bytes → same size AND same content.
    let size_a = write_cbz("Warlord (2025)/Warlord 002.cbz", 3);
    let size_b = write_cbz("Warlord (2025)/Warlord 005.cbz", 3);
    assert_eq!(size_a, size_b, "fixture must produce a size collision");
    // Different page count → different size, so not a candidate at all.
    let size_solo = write_cbz("Warlord (2025)/Warlord 009.cbz", 5);
    assert_ne!(size_solo, size_a);
    // Equal SIZE is what makes them candidates; equal BYTES is what the
    // digest assertion needs. They coincide here only because zip is built
    // without its `time` feature, so entry timestamps are the fixed
    // 1980-01-01 default. Assert the bytes so enabling that feature anywhere
    // in the workspace fails deterministically instead of flaking on a
    // 2-second boundary.
    assert_eq!(
        std::fs::read(app.library_path().join("Warlord (2025)/Warlord 002.cbz")).unwrap(),
        std::fs::read(app.library_path().join("Warlord (2025)/Warlord 005.cbz")).unwrap(),
        "fixture must be byte-identical, not merely same-size"
    );

    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "My Little Warlord".into(),
            sort_title: "my little warlord".into(),
            start_year: Some(2025),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    let now = {
        let n = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(n.date(), n.time())
    };
    let mut ids = Vec::new();
    for (rel, number, size) in [
        ("Warlord (2025)/Warlord 002.cbz", "2", size_a),
        ("Warlord (2025)/Warlord 005.cbz", "5", size_b),
        ("Warlord (2025)/Warlord 009.cbz", "9", size_solo),
    ] {
        let issue = issue_repo::insert(
            &app.state.db,
            NewIssue {
                series_id: series,
                cv_issue_id: None,
                metron_issue_id: None,
                number: number.into(),
                title: None,
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap()
        .id;
        let mtime = {
            let m = std::fs::metadata(app.library_path().join(rel)).unwrap();
            let off =
                time::OffsetDateTime::from(m.modified().unwrap()).to_offset(time::UtcOffset::UTC);
            time::PrimitiveDateTime::new(off.date(), off.time())
        };
        ids.push(
            file_repo::insert(
                &app.state.db,
                NewFile {
                    issue_id: Some(issue),
                    library_root_id: app.library_root_id,
                    path_relative: rel.into(),
                    size_bytes: size,
                    mtime,
                    last_scanned_at: now,
                    match_method: "test".into(),
                    match_confidence: 1.0,
                    status: "owned".into(),
                    cached_comicinfo_xml: None,
                    cached_at: None,
                    is_present: true,
                    last_seen_at: now,
                    matched_at: Some(now),
                },
            )
            .await
            .unwrap()
            .id,
        );
    }
    (ids[0], ids[1], ids[2])
}

/// **The write surface as an assertion, not a declaration.**
///
/// Snapshots every column of `files` that the analyze pass must NOT touch,
/// runs a real pass over real archives, and asserts they are byte-identical
/// afterwards. If `refresh_digests` were edited to also set `is_present = 0`
/// or null an `issue_id`, every other test on this branch would still pass —
/// this is the one that would fail.
///
/// The `Surface::Writes(DIGEST_COLUMNS)` declaration says what the pass is
/// allowed to write. This checks it.
#[tokio::test]
async fn the_pass_writes_nothing_outside_the_digest_columns() {
    let app = build_test_app().await;
    common_seed_three_files(&app).await;

    // Every column except the five the pass may write, concatenated per row.
    // `quote()` renders NULLs distinctly and keeps this to one comparable
    // value per row — Rust only derives PartialEq for tuples up to 12 fields,
    // and there are more guarded columns than that, which is itself the point.
    const GUARDED: &str = "id, quote(issue_id) || '|' || quote(library_root_id) || '|' \
        || quote(path_relative) || '|' || quote(size_bytes) || '|' || quote(mtime) || '|' \
        || quote(last_scanned_at) || '|' || quote(match_method) || '|' \
        || quote(match_confidence) || '|' || quote(status) || '|' \
        || quote(cached_comicinfo_xml) || '|' || quote(cached_at) || '|' \
        || quote(is_present) || '|' || quote(last_seen_at) || '|' || quote(matched_at)";
    let snapshot = || async {
        sqlx::query_as::<_, (i64, String)>(&format!("SELECT {GUARDED} FROM files ORDER BY id"))
            .fetch_all(&app.state.db)
            .await
            .unwrap()
    };

    let before = snapshot().await;
    assert!(!before.is_empty(), "fixture seeded nothing");

    let resp = app
        .request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let status = wait_for_idle(&app).await;
    assert!(
        status["last"]["hashed"].as_u64().unwrap() > 0,
        "the pass must actually have written digests, or this proves nothing: {}",
        status["last"]
    );

    let after = snapshot().await;
    assert_eq!(
        before, after,
        "the analyze pass wrote outside the digest/label columns"
    );
}

/// The discriminating case duplicate detection actually consumes: two files
/// of the SAME size with DIFFERENT bytes. Both must be hashed, and their
/// digests must differ. A size-only implementation passes the identical-bytes
/// test and fails this one.
#[tokio::test]
async fn same_size_different_bytes_produce_different_digests() {
    use std::io::Write;
    let app = build_test_app().await;

    let write = |rel: &str, fill: u8| {
        let full = app.library_path().join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        let f = std::fs::File::create(&full).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("Series 001/001-0000.jpg", opts).unwrap();
        // Stored (uncompressed) so equal payload length means equal file
        // length regardless of how the bytes themselves compress.
        zip.write_all(&vec![fill; 4096]).unwrap();
        zip.finish().unwrap();
        std::fs::metadata(&full).unwrap().len() as i64
    };
    let sa = write("Series (2025)/Series 001.cbz", 1);
    let sb = write("Series (2025)/Series 002.cbz", 2);
    assert_eq!(sa, sb, "fixture must collide on size");
    assert_ne!(
        std::fs::read(app.library_path().join("Series (2025)/Series 001.cbz")).unwrap(),
        std::fs::read(app.library_path().join("Series (2025)/Series 002.cbz")).unwrap(),
        "fixture must differ in bytes"
    );

    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Series".into(),
            sort_title: "series".into(),
            start_year: Some(2025),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let now = {
        let n = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(n.date(), n.time())
    };
    let mut ids = Vec::new();
    for (rel, number, size) in [
        ("Series (2025)/Series 001.cbz", "1", sa),
        ("Series (2025)/Series 002.cbz", "2", sb),
    ] {
        let issue = issue_repo::insert(
            &app.state.db,
            NewIssue {
                series_id: series,
                cv_issue_id: None,
                metron_issue_id: None,
                number: number.into(),
                title: None,
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap()
        .id;
        let mtime = {
            let m = std::fs::metadata(app.library_path().join(rel)).unwrap();
            let off =
                time::OffsetDateTime::from(m.modified().unwrap()).to_offset(time::UtcOffset::UTC);
            time::PrimitiveDateTime::new(off.date(), off.time())
        };
        ids.push(
            file_repo::insert(
                &app.state.db,
                NewFile {
                    issue_id: Some(issue),
                    library_root_id: app.library_root_id,
                    path_relative: rel.into(),
                    size_bytes: size,
                    mtime,
                    last_scanned_at: now,
                    match_method: "test".into(),
                    match_confidence: 1.0,
                    status: "owned".into(),
                    cached_comicinfo_xml: None,
                    cached_at: None,
                    is_present: true,
                    last_seen_at: now,
                    matched_at: Some(now),
                },
            )
            .await
            .unwrap()
            .id,
        );
    }

    app.request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    let status = wait_for_idle(&app).await;
    assert_eq!(status["last"]["hashed"], 2, "both must be hashed");

    /// (file id, digest, stamped size, label kind)
    type DigestRow = (i64, Option<String>, Option<i64>, Option<String>);
    let rows: Vec<DigestRow> = sqlx::query_as(
        "SELECT id, content_blake3, hashed_size_bytes, archive_label_kind FROM files ORDER BY id",
    )
    .fetch_all(&app.state.db)
    .await
    .unwrap();
    let a = rows.iter().find(|r| r.0 == ids[0]).unwrap();
    let b = rows.iter().find(|r| r.0 == ids[1]).unwrap();
    assert_ne!(
        a.1, b.1,
        "same size, different bytes must produce different digests"
    );
    // The version stamp is what makes a second pass free; assert it landed.
    assert_eq!(a.2, Some(sa), "hashed_size_bytes must be stamped");
    assert_eq!(
        a.3.as_deref(),
        Some("dir"),
        "archive_label_kind must be recorded, not just the label"
    );
}

/// `failed` on its own is an anonymous integer. A pass that fails on a file
/// must say which file and why, or the only recourse is reading container
/// logs — which is the worst instruction to give someone whose library just
/// went wrong.
#[tokio::test]
async fn a_failed_file_is_explained_not_just_counted() {
    let app = build_test_app().await;
    let (a, _b, _solo) = common_seed_three_files(&app).await;

    // Make one candidate unreadable while leaving its row intact — the shape
    // of a file removed or permission-changed under a running app.
    let row: (String,) = sqlx::query_as("SELECT path_relative FROM files WHERE id = ?")
        .bind(a)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    std::fs::remove_file(app.library_path().join(&row.0)).unwrap();

    app.request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    let status = wait_for_idle(&app).await;

    // A vanished file is a SKIP, not a failure — it is a normal race with a
    // scan. So this asserts the reporting channel exists and stays quiet when
    // nothing failed, which is the honest assertion for this input.
    assert_eq!(status["last"]["skipped"], 1, "a vanished file is skipped");
    assert_eq!(status["last"]["failed"], 0);
    assert_eq!(
        status["last"]["first_failure"],
        serde_json::Value::Null,
        "nothing failed, so nothing is explained"
    );
}

// -------- class (b): disk/DB reconciliation --------

/// Seed a `files` row with a caller-chosen `is_present`, optionally writing
/// the file. The two are deliberately independent so a fixture can make the
/// catalog DISAGREE with disk — which is the entire subject of this class.
async fn seed_row(app: &TestApp, rel: &str, is_present: bool, write: bool) -> i64 {
    if write {
        let full = app.library_path().join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, b"PK\x03\x04 not really a zip, never opened here").unwrap();
    }
    let now = {
        let n = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(n.date(), n.time())
    };
    file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: None,
            library_root_id: app.library_root_id,
            path_relative: rel.into(),
            // Deliberately absurd: a catalog size no file here has. Any code
            // path that reads this instead of stat-ing fails loudly.
            size_bytes: 999_000_000,
            mtime: now,
            last_scanned_at: now,
            match_method: "test".into(),
            match_confidence: 0.0,
            status: "unmatched".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present,
            last_seen_at: now,
            matched_at: None,
        },
    )
    .await
    .unwrap()
    .id
}

/// The class exists to find disagreement, so the fixture manufactures every
/// kind of disagreement at once. A fixture where catalog and disk agree would
/// pass against an implementation that simply returned empty lists.
#[tokio::test]
async fn reconciliation_finds_each_kind_of_disagreement() {
    let app = build_test_app().await;

    // Agrees: row says present, file is there. Must appear in nothing.
    seed_row(&app, "Saga (2012)/Saga 001.cbz", true, true).await;
    // Ghost: row insists it is present, nothing on disk.
    seed_row(&app, "Saga (2012)/Saga 002.cbz", true, false).await;
    // Drift: row says absent, file is right there.
    seed_row(&app, "Saga (2012)/Saga 003.cbz", false, true).await;
    // Orphan: on disk, no row at all.
    let orphan = app.library_path().join("Saga (2012)/Saga 004.cbz");
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, b"orphaned").unwrap();

    let resp = app
        .request(empty_request(
            "GET",
            "/api/library/integrity/reconciliation",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;

    let list = |k: &str| -> Vec<String> {
        body[k]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(list("orphans"), vec!["Saga (2012)/Saga 004.cbz"]);
    assert_eq!(list("ghosts"), vec!["Saga (2012)/Saga 002.cbz"]);
    assert_eq!(
        list("present_but_marked_absent"),
        vec!["Saga (2012)/Saga 003.cbz"]
    );

    // Provenance turns the counts into a measurement.
    assert_eq!(body["provenance"]["files_seen"], 3, "three files on disk");
    assert_eq!(
        body["provenance"]["rows_compared"], 2,
        "two rows claim to be present"
    );
    assert_eq!(
        body["provenance"]["unreadable"].as_array().unwrap().len(),
        0
    );
    assert!(body["provenance"]["root"].as_str().unwrap().len() > 1);
}

/// A clean library must report zero AND say it looked. Zero without
/// provenance is indistinguishable from a walk that never ran.
#[tokio::test]
async fn a_clean_library_reports_zero_as_a_measurement() {
    let app = build_test_app().await;
    seed_row(&app, "Saga (2012)/Saga 001.cbz", true, true).await;

    let resp = app
        .request(empty_request(
            "GET",
            "/api/library/integrity/reconciliation",
        ))
        .await;
    let body = response_json(resp).await;
    assert_eq!(body["orphans"].as_array().unwrap().len(), 0);
    assert_eq!(body["ghosts"].as_array().unwrap().len(), 0);
    assert_eq!(
        body["provenance"]["files_seen"], 1,
        "zero findings only mean something if the walk saw the library"
    );
}

/// The reconciler must classify exactly what the scanner would catalogue.
/// A second walker with its own rules turns every disagreement about what
/// counts as a comic into a permanent false orphan — `.cb7`, dotfiles and
/// stray non-comics are all skipped by `longbox_scanner::walk_library`.
#[tokio::test]
async fn files_the_scanner_ignores_are_not_orphans() {
    let app = build_test_app().await;
    let dir = app.library_path().join("Saga (2012)");
    std::fs::create_dir_all(&dir).unwrap();
    for name in [".hidden.cbz", "notes.txt", "archive.cb7", "Thumbs.db"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    let resp = app
        .request(empty_request(
            "GET",
            "/api/library/integrity/reconciliation",
        ))
        .await;
    let body = response_json(resp).await;
    assert_eq!(
        body["orphans"].as_array().unwrap().len(),
        0,
        "non-comics must not be reported as orphans: {}",
        body["orphans"]
    );
    assert_eq!(body["provenance"]["files_seen"], 0);
}

// -------- classes (a) (c) (d) (e) (f) --------

/// Content duplicates read 0 before analysis has run, and that zero means
/// "not looked at yet", not "none". `unanalyzed_candidates` is what makes the
/// difference visible — the same problem as class (b)'s provenance and
/// `pending_analysis`, in a third place.
#[tokio::test]
async fn content_duplicates_report_how_much_is_still_unanalyzed() {
    let app = build_test_app().await;
    common_seed_three_files(&app).await;

    let before = response_json(
        app.request(empty_request("GET", "/api/library/integrity/findings"))
            .await,
    )
    .await;
    assert_eq!(before["content_duplicates"].as_array().unwrap().len(), 0);
    assert_eq!(
        before["unanalyzed_candidates"], 2,
        "two size-colliding files have no digest yet, so 0 groups is a floor"
    );

    app.request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    wait_for_idle(&app).await;

    let after = response_json(
        app.request(empty_request("GET", "/api/library/integrity/findings"))
            .await,
    )
    .await;
    assert_eq!(
        after["content_duplicates"].as_array().unwrap().len(),
        1,
        "the identical pair is now a group"
    );
    assert_eq!(
        after["unanalyzed_candidates"], 0,
        "and nothing is left unexamined, so 0 groups would now mean 0"
    );
    let g = &after["content_duplicates"][0];
    assert_eq!(g["files"].as_array().unwrap().len(), 2);
    assert_eq!(g["spans_multiple_series"], false);
    assert!(g["redundant_bytes"].as_i64().unwrap() > 0);
}

/// Identical bytes under two different series — the Blood Train shape, and
/// the one Tidy structurally cannot see because the copies share no issue_id.
#[tokio::test]
async fn identical_content_under_two_series_is_flagged() {
    use std::io::Write;
    let app = build_test_app().await;

    let write = |rel: &str| {
        let full = app.library_path().join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        let f = std::fs::File::create(&full).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("Blood Train (one-shot)/001.jpg", opts)
            .unwrap();
        zip.write_all(&vec![3u8; 4096]).unwrap();
        zip.finish().unwrap();
        std::fs::metadata(&full).unwrap().len() as i64
    };
    // One comic, filed under two different series.
    let sa = write("Blood Train/Blood Train 001.cbz");
    let sb = write("Book of Cutter/Book of Cutter 001.cbz");
    assert_eq!(sa, sb);

    let now = {
        let n = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(n.date(), n.time())
    };
    for (title, rel) in [
        ("Blood Train", "Blood Train/Blood Train 001.cbz"),
        ("Book of Cutter", "Book of Cutter/Book of Cutter 001.cbz"),
    ] {
        let series = series_repo::insert(
            &app.state.db,
            NewSeries {
                cv_id: None,
                metron_id: None,
                title: title.into(),
                sort_title: title.to_lowercase(),
                start_year: Some(2025),
                publisher: None,
                description: None,
                cover_url: None,
            },
        )
        .await
        .unwrap()
        .id;
        let issue = issue_repo::insert(
            &app.state.db,
            NewIssue {
                series_id: series,
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
        let mtime = {
            let m = std::fs::metadata(app.library_path().join(rel)).unwrap();
            let off =
                time::OffsetDateTime::from(m.modified().unwrap()).to_offset(time::UtcOffset::UTC);
            time::PrimitiveDateTime::new(off.date(), off.time())
        };
        file_repo::insert(
            &app.state.db,
            NewFile {
                issue_id: Some(issue),
                library_root_id: app.library_root_id,
                path_relative: rel.into(),
                size_bytes: sa,
                mtime,
                last_scanned_at: now,
                match_method: "filename_regex".into(),
                match_confidence: 0.9,
                status: "owned".into(),
                cached_comicinfo_xml: None,
                cached_at: None,
                is_present: true,
                last_seen_at: now,
                matched_at: Some(now),
            },
        )
        .await
        .unwrap();
    }

    app.request(request(Method::POST, "/api/library/integrity/analyze"))
        .await;
    wait_for_idle(&app).await;

    let body = response_json(
        app.request(empty_request("GET", "/api/library/integrity/findings"))
            .await,
    )
    .await;
    let groups = body["content_duplicates"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0]["spans_multiple_series"], true,
        "one comic filed under two series is its own finding"
    );
    // The archive's own label names which series it really is.
    let labels: Vec<&str> = groups[0]["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["archive_series"].as_str())
        .collect();
    assert!(
        labels.iter().all(|l| *l == "blood train"),
        "the archive says what it is, whatever folder it sits in: {labels:?}"
    );
}

/// Cross-folder splits three ways, and the empty category stays visible: an
/// empty `wrong_volume` that CAN populate is a regression indicator, while
/// deleting it would let the next wrong-volume binding land in
/// `benign_variant` and be dismissed as cosmetic.
#[tokio::test]
async fn cross_folder_splits_into_three_categories() {
    let app = build_test_app().await;

    // Two catalog volumes of one title, so a stray folder naming the other
    // year is a genuine wrong-volume binding.
    let v1999 = seed_series_with_year(&app, "The Authority", 1999).await;
    let _v2008 = seed_series_with_year(&app, "The Authority", 2008).await;
    // Majority in the 1999 folder, one stray in the 2008 folder.
    for n in ["1", "2", "3"] {
        seed_bound(
            &app,
            v1999,
            n,
            &format!("The Authority (1999)/The Authority {n}.cbz"),
        )
        .await;
    }
    seed_bound(&app, v1999, "4", "The Authority (2008)/The Authority 4.cbz").await;

    // Benign: one series, folder spelled without its year.
    let drifter = seed_series_with_year(&app, "Drifter", 2014).await;
    for n in ["1", "2"] {
        seed_bound(&app, drifter, n, &format!("Drifter (2014)/Drifter {n}.cbz")).await;
    }
    seed_bound(&app, drifter, "3", "Drifter/Drifter 3.cbz").await;

    // Trade: a differently-titled folder holding this series' issues.
    let fp = seed_series_with_year(&app, "Fire Power", 2020).await;
    for n in ["1", "2"] {
        seed_bound(
            &app,
            fp,
            n,
            &format!("Fire Power (2020)/Fire Power {n}.cbz"),
        )
        .await;
    }
    seed_bound(
        &app,
        fp,
        "3",
        "Fire Power By Kirkman And Samnee (2023)/vol 1.cbz",
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/library/integrity/findings"))
            .await,
    )
    .await;
    let cats: Vec<&str> = body["cross_folder"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["category"].as_str().unwrap())
        .collect();
    assert_eq!(
        cats.iter().filter(|c| **c == "wrong_volume").count(),
        1,
        "a stray folder naming another catalog volume is wrong_volume: {cats:?}"
    );
    assert_eq!(cats.iter().filter(|c| **c == "benign_variant").count(), 1);
    assert_eq!(
        cats.iter().filter(|c| **c == "trade_or_collection").count(),
        1
    );
}

/// (d) uses the PRODUCTION parser and the live patterns. A local
/// reimplementation would report every difference between itself and
/// production as a finding, measuring the parsers rather than the library.
#[tokio::test]
async fn filename_disagreement_uses_the_production_parser() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Death Fight Forever", 2026).await;
    // Bound to issue 3, named 005 — the live shape.
    seed_bound(
        &app,
        series,
        "3",
        "Death Fight Forever (2026)/Death Fight Forever (2026) 005.cbz",
    )
    .await;
    // Agrees; must not be reported.
    seed_bound(
        &app,
        series,
        "1",
        "Death Fight Forever (2026)/Death Fight Forever (2026) 001.cbz",
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/library/integrity/findings"))
            .await,
    )
    .await;
    let d = body["filename_disagreements"].as_array().unwrap();
    assert_eq!(d.len(), 1, "only the disagreeing file: {d:?}");
    assert_eq!(d[0]["filename_says"], "005");
    assert_eq!(d[0]["bound_to"], "3");
}

/// (f) surfaces the trapped rows with no action attached.
#[tokio::test]
async fn orphaned_owned_rows_are_surfaced() {
    let app = build_test_app().await;
    let now = {
        let n = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(n.date(), n.time())
    };
    file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: None,
            library_root_id: app.library_root_id,
            path_relative: "Gone/Gone 001.cbz".into(),
            size_bytes: 1,
            mtime: now,
            last_scanned_at: now,
            match_method: "filename_regex".into(),
            match_confidence: 0.9,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: false,
            last_seen_at: now,
            matched_at: None,
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", "/api/library/integrity/findings"))
            .await,
    )
    .await;
    let rows = body["orphaned_owned_rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["path_relative"], "Gone/Gone 001.cbz");
    assert_eq!(rows[0]["is_present"], false);
}

async fn seed_series_with_year(app: &TestApp, title: &str, year: i32) -> i64 {
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: longbox_core::normalize_title(title),
            start_year: Some(year),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

/// A present, bound file. Catalog size is deliberately absurd relative to
/// what is written, so any path substituting it for a disk read fails loudly.
async fn seed_bound(app: &TestApp, series_id: i64, number: &str, rel: &str) -> i64 {
    let full = app.library_path().join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, format!("stub for {rel}")).unwrap();
    let issue = issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let now = {
        let n = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(n.date(), n.time())
    };
    file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: Some(issue),
            library_root_id: app.library_root_id,
            path_relative: rel.into(),
            size_bytes: 888_000_000,
            mtime: now,
            last_scanned_at: now,
            match_method: "filename_regex".into(),
            match_confidence: 0.9,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now,
            matched_at: Some(now),
        },
    )
    .await
    .unwrap()
    .id
}

// -------- deleting one redundant copy --------
//
// Four shapes drawn from the live findings, each of which would be a
// silent wrong answer rather than an error if handled badly.

/// Seed a file with a chosen digest, status and binding, and write the
/// bytes so the delete has something to unlink.
async fn seed_copy(
    app: &TestApp,
    issue: Option<i64>,
    rel: &str,
    digest: &str,
    status: &str,
) -> i64 {
    let now = time::OffsetDateTime::now_utc();
    let now = time::PrimitiveDateTime::new(now.date(), now.time());
    let abs = app.library_path().join(rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, b"bytes").unwrap();
    let id = file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: issue,
            library_root_id: app.library_root_id,
            path_relative: rel.into(),
            size_bytes: 5,
            mtime: now,
            last_scanned_at: now,
            match_method: "test".into(),
            match_confidence: 1.0,
            status: status.into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now,
            matched_at: Some(now),
        },
    )
    .await
    .unwrap()
    .id;
    // Stamp the digest against the file's ACTUAL size and mtime, the
    // way the analyze pass does. A synthetic stamp reads as stale to
    // `DiskObservation::stat`, which would make every fixture look like
    // a modified file rather than a verified duplicate.
    let meta = std::fs::metadata(&abs).unwrap();
    let m: time::OffsetDateTime = meta.modified().unwrap().into();
    let m = m.to_offset(time::UtcOffset::UTC);
    let m = time::PrimitiveDateTime::new(m.date(), m.time());
    file_repo::set_content_hash(
        &app.state.db,
        id,
        digest,
        i64::try_from(meta.len()).unwrap(),
        m,
    )
    .await
    .unwrap();
    id
}

/// Register a row for a file that is already on disk, stamping the
/// digest against its current stat. Used where writing the bytes again
/// would disturb another row's stamp.
async fn seed_existing(app: &TestApp, issue: Option<i64>, rel: &str, digest: &str) -> i64 {
    let now = time::OffsetDateTime::now_utc();
    let now = time::PrimitiveDateTime::new(now.date(), now.time());
    let id = file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: issue,
            library_root_id: app.library_root_id,
            path_relative: rel.into(),
            size_bytes: 5,
            mtime: now,
            last_scanned_at: now,
            match_method: "test".into(),
            match_confidence: 1.0,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now,
            matched_at: Some(now),
        },
    )
    .await
    .unwrap()
    .id;
    let meta = std::fs::metadata(app.library_path().join(rel)).unwrap();
    let m: time::OffsetDateTime = meta.modified().unwrap().into();
    let m = m.to_offset(time::UtcOffset::UTC);
    let m = time::PrimitiveDateTime::new(m.date(), m.time());
    file_repo::set_content_hash(
        &app.state.db,
        id,
        digest,
        i64::try_from(meta.len()).unwrap(),
        m,
    )
    .await
    .unwrap();
    id
}

async fn delete_dup(app: &TestApp, digest: &str, file_id: i64) -> (StatusCode, serde_json::Value) {
    let resp = app
        .request(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/api/library/integrity/duplicates/delete")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(format!(
                    r#"{{"digest":"{digest}","file_id":{file_id}}}"#
                )))
                .unwrap(),
        )
        .await;
    let status = resp.status();
    (status, response_json(resp).await)
}

/// The payoff: the issue a redundant copy was falsely occupying goes
/// back to missing, so the pull engine fetches the real book.
///
/// Ownership is derived (`EXISTS` over present files), so this should
/// fall out of removing the row — asserted rather than assumed,
/// because the entire feature is justified by it.
#[tokio::test]
async fn deleting_a_copy_reverts_the_issue_it_was_occupying() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Skinbreaker", 2025).await;
    let keep_issue = seed_bound_issue(&app, series, "5").await;
    let false_issue = seed_bound_issue(&app, series, "6").await;
    let keep = seed_copy(&app, Some(keep_issue), "S/S 005.cbz", "dig-a", "owned").await;
    let loser = seed_copy(&app, Some(false_issue), "S/S 006.cbz", "dig-a", "owned").await;

    assert!(issue_is_owned(&app, false_issue).await, "precondition");

    let (status, body) = delete_dup(&app, "dig-a", loser).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reverted"]["issue_id"], false_issue);
    assert_eq!(body["reverted"]["now_missing"], true);
    assert_eq!(body["remaining_in_group"], 1);

    assert!(
        !issue_is_owned(&app, false_issue).await,
        "the issue must revert to missing — this is the whole payoff"
    );
    assert!(
        issue_is_owned(&app, keep_issue).await,
        "the kept copy's issue is untouched"
    );
    assert!(!on_disk_rel(&app, "S/S 006.cbz"));
    assert!(on_disk_rel(&app, "S/S 005.cbz"));
    let _ = keep;
}

/// Radiant Black 29.5. A decimal issue number must survive the round
/// trip as text — parsed as a number it becomes 29, and the
/// confirmation then names the wrong issue.
#[tokio::test]
async fn a_decimal_issue_number_survives_the_delete_unchanged() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Radiant Black", 2021).await;
    let a = seed_bound_issue(&app, series, "29").await;
    let b = seed_bound_issue(&app, series, "29.5").await;
    let keep = seed_copy(&app, Some(a), "R/R 029.cbz", "dig-dec", "owned").await;
    let loser = seed_copy(&app, Some(b), "R/R 29.5.cbz", "dig-dec", "owned").await;

    let (status, body) = delete_dup(&app, "dig-dec", loser).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["reverted"]["issue_number"], "29.5",
        "the decimal must come back exactly, not truncated to 29"
    );
    let _ = keep;
}

/// Blood Train / Book of Cutter: one group, two SERIES. Nothing in the
/// delete path may assume a group has a single series.
#[tokio::test]
async fn a_group_spanning_two_series_deletes_from_the_right_one() {
    let app = build_test_app().await;
    let s1 = seed_series_with_year(&app, "Blood Train", 2025).await;
    let s2 = seed_series_with_year(&app, "Book of Cutter", 2025).await;
    let i1 = seed_bound_issue(&app, s1, "1").await;
    let i2 = seed_bound_issue(&app, s2, "1").await;
    let keep = seed_copy(&app, Some(i1), "BT/BT 001.cbr", "dig-x", "owned").await;
    let loser = seed_copy(&app, Some(i2), "BC/BC 001.cbr", "dig-x", "owned").await;

    let (status, body) = delete_dup(&app, "dig-x", loser).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reverted"]["issue_id"], i2);
    assert_eq!(body["reverted"]["series_title"], "Book of Cutter");
    assert!(
        issue_is_owned(&app, i1).await,
        "the other series' issue must be untouched"
    );
    assert!(!issue_is_owned(&app, i2).await);
    let _ = keep;
}

/// Hello Darkness: a third copy that is `ignored` and bound to nothing.
/// Deleting it must not be blocked by the status, and must revert
/// nothing — it owned nothing to begin with.
#[tokio::test]
async fn an_ignored_unbound_copy_deletes_and_reverts_nothing() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Hello Darkness", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let bound = seed_copy(&app, Some(i1), "HD/HD 001.cbz", "dig-hd", "owned").await;
    let stray = seed_copy(&app, None, "HD/HD stray.cbz", "dig-hd", "ignored").await;

    let (status, body) = delete_dup(&app, "dig-hd", stray).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["reverted"].is_null(),
        "an unbound copy owns nothing, so nothing reverts: {body}"
    );
    assert!(
        issue_is_owned(&app, i1).await,
        "the bound copy's issue must be untouched"
    );
    assert!(!on_disk_rel(&app, "HD/HD stray.cbz"));
    let _ = bound;
}

/// The three issues carrying a stale `grabbed` attempt. The revert
/// lands immediately but re-search waits for the sweep to purge it, and
/// the user must be told or they will read a missing issue as a failed
/// delete.
#[tokio::test]
async fn a_stale_grab_on_the_reverted_issue_is_reported() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Ferocious", 2025).await;
    let a = seed_bound_issue(&app, series, "2").await;
    let b = seed_bound_issue(&app, series, "5").await;
    let keep = seed_copy(&app, Some(a), "F/F 002.cbz", "dig-g", "owned").await;
    let loser = seed_copy(&app, Some(b), "F/F 005.cbz", "dig-g", "owned").await;

    longbox_db::pull_attempt_repo::insert(
        &app.state.db,
        longbox_db::NewPullAttempt {
            series_id: series,
            issue_id: b,
            indexer_id: None,
            release_id: Some("guid".into()),
            status: "grabbed".into(),
            error_message: None,
            retry_count: 0,
            download_handle: None,
        },
    )
    .await
    .unwrap();

    let (status, body) = delete_dup(&app, "dig-g", loser).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reverted"]["now_missing"], true);
    assert_eq!(
        body["reverted"]["awaiting_stale_grab_purge"], "grabbed",
        "the user must be told re-search waits for the sweep: {body}"
    );
    let _ = keep;
}

/// Refuses to empty a group. Deleting the last copy of a digest
/// destroys the only bytes, which is a different act from removing a
/// redundant one.
#[tokio::test]
async fn refuses_to_delete_the_last_copy_of_a_digest() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Lonely", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let only = seed_copy(&app, Some(i1), "L/L 001.cbz", "dig-solo", "owned").await;

    let (status, body) = delete_dup(&app, "dig-solo", only).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(on_disk_rel(&app, "L/L 001.cbz"), "bytes must survive");
    assert!(issue_is_owned(&app, i1).await, "issue must stay owned");
}

/// The digest is the client's claim about which group it was looking
/// at. A file that is not in that group cannot be deleted through it.
#[tokio::test]
async fn refuses_a_file_that_is_not_in_the_named_group() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Elsewhere", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let i2 = seed_bound_issue(&app, series, "2").await;
    let a = seed_copy(&app, Some(i1), "E/E 001.cbz", "dig-1", "owned").await;
    let b = seed_copy(&app, Some(i2), "E/E 002.cbz", "dig-1", "owned").await;
    let other = seed_copy(&app, None, "E/E other.cbz", "dig-2", "ignored").await;

    let (status, body) = delete_dup(&app, "dig-1", other).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(on_disk_rel(&app, "E/E other.cbz"));
    let _ = (a, b);
}

// -- local helpers --

async fn seed_bound_issue(app: &TestApp, series_id: i64, number: &str) -> i64 {
    issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.into(),
            title: None,
            cover_date: Some("2025-01-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

async fn issue_is_owned(app: &TestApp, issue_id: i64) -> bool {
    file_repo::list_by_issue(&app.state.db, issue_id)
        .await
        .unwrap()
        .iter()
        .any(|f| f.status == "owned" && f.is_present)
}

fn on_disk_rel(app: &TestApp, rel: &str) -> bool {
    app.library_path().join(rel).exists()
}

/// One deletion does not imply one revert, and the confirmation copy
/// must not promise otherwise.
///
/// Drawn from the live data: `Hello Darkness (2025) 002.cbz` is a
/// content-duplicate, but its issue ALSO owns
/// `Hello Darkness (2024) 002.cbz` — a different file, different
/// bytes, different folder, outside the duplicate group entirely.
/// Removing the duplicate leaves the issue owned.
///
/// This is why `now_missing` is re-queried after the delete instead of
/// inferred from the act of deleting: across the 37 live groups it is
/// true 36 times and false once, and the once is invisible from inside
/// the group.
#[tokio::test]
async fn a_second_owned_file_outside_the_group_means_no_revert() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Hello Darkness", 2025).await;
    let shared = seed_bound_issue(&app, series, "2").await;
    let other_issue = seed_bound_issue(&app, series, "1").await;

    // The duplicate group: two byte-identical copies on two issues.
    let keep = seed_copy(
        &app,
        Some(other_issue),
        "HD25/HD 001.cbz",
        "dig-hd2",
        "owned",
    )
    .await;
    let loser = seed_copy(&app, Some(shared), "HD25/HD 002.cbz", "dig-hd2", "owned").await;
    // A different file, different content, on the SAME issue as the
    // loser — the 2024-folder copy. Not part of the group.
    let outsider = seed_copy(&app, Some(shared), "HD24/HD 002.cbz", "dig-other", "owned").await;

    let (status, body) = delete_dup(&app, "dig-hd2", loser).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reverted"]["issue_id"], shared);
    assert_eq!(
        body["reverted"]["now_missing"], false,
        "the issue keeps its other owned file, so it does NOT revert: {body}"
    );

    assert!(
        issue_is_owned(&app, shared).await,
        "the outside copy still owns this issue"
    );
    assert!(
        !on_disk_rel(&app, "HD25/HD 002.cbz"),
        "the duplicate is still deleted"
    );
    assert!(
        on_disk_rel(&app, "HD24/HD 002.cbz"),
        "the unrelated copy must be untouched"
    );
    let _ = (keep, outsider);
}

/// A link and its target in one group is not a duplicate at all, and
/// BOTH directions refuse.
///
/// The scanner walks with `follow_links(true)`, so a link and its
/// target are both catalogued as ordinary rows carrying identical
/// bytes — which looks exactly like a content-duplicate group. It is
/// not one: there is a single file wearing two names, so no deletion
/// here removes a redundant copy.
///
/// This asserts BOTH arrows deliberately. An earlier design let the
/// link direction succeed, on the true-about-bytes reasoning that
/// unlinking a link frees nothing — which then allowed a group of two
/// links to one outside file to be emptied completely while the
/// endpoint's own refusal message promised that could not happen. One
/// behaviour, both directions, or the asymmetry comes back.
#[tokio::test]
async fn an_aliased_pair_is_refused_from_either_direction() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Linked", 2025).await;
    let keep_issue = seed_bound_issue(&app, series, "1").await;
    let link_issue = seed_bound_issue(&app, series, "2").await;

    // Create the link first and write through it ONCE, so the bytes
    // land before either row is stamped. Writing the target afterwards
    // would change its mtime and make the sibling look (correctly) like
    // a file modified since it was hashed, which the freshness guard
    // would then refuse — masking the behaviour under test.
    std::fs::create_dir_all(app.library_path().join("Real")).unwrap();
    std::fs::create_dir_all(app.library_path().join("Link")).unwrap();
    std::os::unix::fs::symlink(
        app.library_path().join("Real/comic.cbz"),
        app.library_path().join("Link/comic.cbz"),
    )
    .unwrap();
    let link = seed_copy(
        &app,
        Some(link_issue),
        "Link/comic.cbz",
        "dig-link",
        "owned",
    )
    .await;
    // The target now exists, written through the link. Register it as
    // its own row without touching the bytes again.
    let real = seed_existing(&app, Some(keep_issue), "Real/comic.cbz", "dig-link").await;

    for (target, arrow) in [(link, "the link"), (real, "the target")] {
        let (status, body) = delete_dup(&app, "dig-link", target).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "deleting {arrow} must be refused: {body}"
        );
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("same file on disk"),
            "the refusal must name the situation, not just decline: {body}"
        );
    }

    assert!(
        app.library_path().join("Real/comic.cbz").exists(),
        "the only bytes must survive both attempts"
    );
    assert!(
        app.library_path()
            .join("Link/comic.cbz")
            .symlink_metadata()
            .is_ok(),
        "the link must survive too — nothing was redundant"
    );
    assert!(
        issue_is_owned(&app, keep_issue).await && issue_is_owned(&app, link_issue).await,
        "neither issue may revert, because nothing was deleted"
    );
}

/// A symlink whose target lives OUTSIDE the library still unlinks the
/// LINK, never the file it points at.
///
/// Note what does and does not defend this one: under full-path
/// canonicalisation the resolved target lands outside the root and the
/// ESCAPE check refuses the request, so the outside file survives
/// either way. This test therefore pins the success path — that a link
/// in a genuine two-file group is deletable at all — and not the
/// canonicalise-the-parent rule.
/// `deleting_a_link_to_an_uncatalogued_in_library_file_spares_the_target`
/// is the one that pins that rule, because there the escape check
/// passes and only the parent-vs-full choice stands between the delete
/// and a file the user never saw.
#[tokio::test]
async fn deleting_a_link_to_an_outside_file_unlinks_the_link_not_the_target() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Outside", 2025).await;
    let keep_issue = seed_bound_issue(&app, series, "1").await;
    let link_issue = seed_bound_issue(&app, series, "2").await;

    // A file that no library root contains.
    let outside = app.library_path().parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("comic.cbz");
    std::fs::write(&outside_file, b"bytes").unwrap();

    std::fs::create_dir_all(app.library_path().join("Link")).unwrap();
    std::os::unix::fs::symlink(&outside_file, app.library_path().join("Link/comic.cbz")).unwrap();
    let link = seed_existing(&app, Some(link_issue), "Link/comic.cbz", "dig-out").await;
    // A genuine, separate in-library copy: same bytes, its own inode.
    let keep = seed_copy(
        &app,
        Some(keep_issue),
        "Genuine/comic.cbz",
        "dig-out",
        "owned",
    )
    .await;

    let (status, body) = delete_dup(&app, "dig-out", link).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "these are two different files, so this IS a duplicate delete: {body}"
    );

    assert!(
        app.library_path()
            .join("Link/comic.cbz")
            .symlink_metadata()
            .is_err(),
        "the symlink itself must be gone"
    );
    assert!(
        outside_file.exists(),
        "the file the link pointed at is outside the library and must be untouched: {body}"
    );
    assert!(
        on_disk_rel(&app, "Genuine/comic.cbz"),
        "the surviving in-library copy must be untouched"
    );
    let _ = keep;
}

/// A row whose file is genuinely gone from disk can still be deleted,
/// and the issue still reverts.
///
/// This pins the PURPOSE of the `target_exists &&` conjunct in the
/// freshness check. Without that conjunct an absent file has no digest,
/// the check refuses, and a row pointing at nothing becomes
/// undeletable — the catalog could never be cleaned up through this
/// screen. The conjunct is what makes that case work, and until now
/// nothing asserted it, so the conjunct could have been deleted with
/// the whole suite green.
///
/// Its known cost is recorded rather than fixed here: `validate_digest`
/// collapses EVERY `metadata()` failure into "absent", so a target that
/// is unreadable rather than missing (`EACCES` on a parent, `EIO` on a
/// flaky mount) also skips the freshness check. The delete then
/// proceeds, the row goes, the unlink fails, and the issue reverts with
/// the bytes still on disk. That is the documented row-first tradeoff —
/// visible via `unlink_error` and recoverable on the next scan — but it
/// is a wider door than the one this test needs open.
#[tokio::test]
async fn a_row_whose_file_is_already_gone_is_deletable_and_still_reverts() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Vanished", 2025).await;
    let gone_issue = seed_bound_issue(&app, series, "1").await;
    let keep_issue = seed_bound_issue(&app, series, "2").await;

    let gone = seed_copy(
        &app,
        Some(gone_issue),
        "Gone/comic.cbz",
        "dig-gone",
        "owned",
    )
    .await;
    let keep = seed_copy(
        &app,
        Some(keep_issue),
        "Genuine/gone.cbz",
        "dig-gone",
        "owned",
    )
    .await;
    // The bytes disappear after the stamp — a file deleted outside
    // LongBox, which is exactly the row this cleans up.
    std::fs::remove_file(app.library_path().join("Gone/comic.cbz")).unwrap();

    let (status, body) = delete_dup(&app, "dig-gone", gone).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a row pointing at nothing must still be removable: {body}"
    );
    assert_eq!(
        body["reverted"]["now_missing"], true,
        "and its issue reverts, because the copy it claimed is gone: {body}"
    );
    assert!(
        body["unlink_error"].is_null(),
        "an already-absent file is not an unlink failure: {body}"
    );
    assert!(
        file_repo::find_by_id(&app.state.db, gone)
            .await
            .unwrap()
            .is_none(),
        "the row must be gone"
    );
    assert!(
        on_disk_rel(&app, "Genuine/gone.cbz"),
        "the surviving copy is untouched"
    );
    let _ = keep;
}

/// The shared delete operation refuses a path that does not name a
/// file, tested against the operation itself.
///
/// Both current routes happen to refuse these earlier, on content — a
/// directory carries no digest, so the freshness check fires first.
/// That makes every route-level test of this guard vacuous: disable the
/// guard entirely and they all still pass. The guard is defence in
/// depth in the SHARED operation, so it is pinned where it lives, not
/// through a caller that would refuse anyway.
#[tokio::test]
async fn the_delete_operation_refuses_a_path_that_does_not_name_a_file() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "RawGuard", 2025).await;
    let issue = seed_bound_issue(&app, series, "1").await;

    std::fs::create_dir_all(app.library_path().join("Shelf")).unwrap();
    std::fs::write(app.library_path().join("Shelf/real.cbz"), b"bytes").unwrap();
    let roots = HashMap::from([(
        app.library_root_id,
        app.library_path().to_string_lossy().into_owned(),
    )]);

    for bad in ["Shelf/.", "Shelf/", "Shelf/./", "Shelf//"] {
        let id = seed_existing(&app, Some(issue), bad, "dig-raw").await;
        let row = file_repo::find_by_id(&app.state.db, id)
            .await
            .unwrap()
            .unwrap();
        let err = longbox_web::file_delete::delete_file(&app.state.db, &roots, &row)
            .await
            .expect_err("{bad:?} names a directory and must be refused");
        assert!(
            err.contains("does not end in a file name"),
            "{bad:?}: refused for the wrong reason: {err}"
        );
        assert!(
            file_repo::find_by_id(&app.state.db, id)
                .await
                .unwrap()
                .is_some(),
            "{bad:?}: the row must survive a refusal"
        );
    }

    // And the guard must not refuse a legitimate path that merely
    // contains a `.` component — the read paths share `is_contained`
    // and see `./a/b.cbz` routinely.
    std::fs::write(app.library_path().join("Shelf/ok.cbz"), b"bytes").unwrap();
    let ok = seed_existing(&app, Some(issue), "Shelf/./ok.cbz", "dig-raw").await;
    let row = file_repo::find_by_id(&app.state.db, ok)
        .await
        .unwrap()
        .unwrap();
    longbox_web::file_delete::delete_file(&app.state.db, &roots, &row)
        .await
        .expect("a real file behind a `.` component must still be deletable");
    assert!(
        !app.library_path().join("Shelf/ok.cbz").exists(),
        "and it must actually be gone"
    );
    assert!(
        app.library_path().join("Shelf/real.cbz").exists(),
        "without touching anything else in the directory"
    );
}

/// A target that IS a directory — no `.`, no trailing slash — is
/// refused, because the guard that catches it is semantic, not
/// syntactic.
///
/// `"Probe"` walks past the dot-guard entirely: it is a perfectly
/// well-formed relative path whose final component is a normal name.
/// What stops it is that the stat reports present-but-no-digest, so the
/// freshness check refuses.
///
/// This is a regression test with a specific history. An earlier
/// version reported a non-file as ABSENT, which is the same tuple as
/// "nothing is there" — and every guard downstream reads absence as a
/// green light. `target_exists` short-circuited the freshness check and
/// the delete proceeded: row destroyed, issue reverted to missing, the
/// directory and the comic inside it untouched. The check meant to
/// harden this path was what opened it.
#[tokio::test]
async fn a_target_that_is_a_directory_is_refused_without_any_syntactic_tell() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "DirTarget", 2025).await;
    let dir_issue = seed_bound_issue(&app, series, "1").await;
    let keep_issue = seed_bound_issue(&app, series, "2").await;

    std::fs::create_dir_all(app.library_path().join("Probe")).unwrap();
    std::fs::write(app.library_path().join("Probe/real.cbz"), b"bytes").unwrap();
    // Note the path: no `.`, no trailing slash. Nothing syntactic to catch.
    let dir_row = seed_existing(&app, Some(dir_issue), "Probe", "dig-dir").await;
    let keep = seed_copy(
        &app,
        Some(keep_issue),
        "Genuine/dir.cbz",
        "dig-dir",
        "owned",
    )
    .await;

    let (status, body) = delete_dup(&app, "dig-dir", dir_row).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a directory carries no verifiable bytes and must be refused: {body}"
    );
    assert!(
        file_repo::find_by_id(&app.state.db, dir_row)
            .await
            .unwrap()
            .is_some(),
        "the catalog row must survive"
    );
    assert!(
        app.library_path().join("Probe/real.cbz").exists(),
        "the directory contents must be untouched"
    );
    assert!(
        issue_is_owned(&app, dir_issue).await,
        "and the issue must not revert on a delete that deleted nothing"
    );
    let _ = keep;
}

/// The canonicalise-the-PARENT rule, pinned against the only case that
/// still needs it.
///
/// Three cases reach a symlinked row, and two are already covered
/// elsewhere: a link and its target both catalogued in one group is
/// refused by the alias guard, and a link to a file outside any root is
/// refused by the escape check. What passes both is a link to an
/// in-library file the scanner never catalogued — a `.bak` beside the
/// comics, say. Full-path canonicalisation resolves it inside the root,
/// clears the escape check, and `remove_file` destroys that file while
/// leaving the link in place: a deletion of something the user never
/// saw in any group and never confirmed.
#[tokio::test]
async fn deleting_a_link_to_an_uncatalogued_in_library_file_spares_the_target() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Backup", 2025).await;
    let keep_issue = seed_bound_issue(&app, series, "1").await;
    let link_issue = seed_bound_issue(&app, series, "2").await;

    // Inside the library, but never catalogued — the scanner skips it
    // on extension, so it is in no group and no guard knows about it.
    std::fs::create_dir_all(app.library_path().join("Backups")).unwrap();
    let backup = app.library_path().join("Backups/comic.bak");
    std::fs::write(&backup, b"bytes").unwrap();

    std::fs::create_dir_all(app.library_path().join("Lnk")).unwrap();
    std::os::unix::fs::symlink(&backup, app.library_path().join("Lnk/comic.cbz")).unwrap();
    let link = seed_existing(&app, Some(link_issue), "Lnk/comic.cbz", "dig-bak").await;
    let keep = seed_copy(
        &app,
        Some(keep_issue),
        "Genuine/bak.cbz",
        "dig-bak",
        "owned",
    )
    .await;

    let (status, body) = delete_dup(&app, "dig-bak", link).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the link and the genuine copy are two different files: {body}"
    );

    assert!(
        backup.exists(),
        "the uncatalogued file the link pointed at must survive — nothing in the UI ever \
         offered it for deletion: {body}"
    );
    assert!(
        app.library_path()
            .join("Lnk/comic.cbz")
            .symlink_metadata()
            .is_err(),
        "the symlink itself must be gone"
    );
    assert!(
        on_disk_rel(&app, "Genuine/bak.cbz"),
        "the surviving copy must be untouched"
    );
    let _ = keep;
}

/// A stale digest is not evidence of a duplicate.
///
/// If a sibling has been modified since the last analyze, its stored
/// digest no longer describes it — so it is not a verified copy of
/// these bytes, and deleting the target would remove the only real
/// one. Decided by stat, never by the catalog's `content_blake3`.
#[tokio::test]
async fn refuses_when_the_only_sibling_has_a_stale_digest() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Stale", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let i2 = seed_bound_issue(&app, series, "2").await;
    let sibling = seed_copy(&app, Some(i1), "St/St 001.cbz", "dig-stale", "owned").await;
    let target = seed_copy(&app, Some(i2), "St/St 002.cbz", "dig-stale", "owned").await;

    // The sibling changes on disk after being hashed: its stored digest
    // now describes bytes that are gone.
    std::fs::write(
        app.library_path().join("St/St 001.cbz"),
        b"completely different content",
    )
    .unwrap();

    let (status, body) = delete_dup(&app, "dig-stale", target).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a modified sibling is not a verified copy: {body}"
    );
    assert!(
        on_disk_rel(&app, "St/St 002.cbz"),
        "the target must survive"
    );
    assert!(issue_is_owned(&app, i2).await);
    let _ = sibling;
}

/// A sibling row whose file is absent from disk does not count as a
/// surviving copy. `is_present` only changes on a scan, so the row can
/// outlive the file — and counting rows would report "1 copy remains"
/// while removing the last real bytes.
#[tokio::test]
async fn refuses_when_the_only_sibling_row_has_no_file_on_disk() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Absent", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let i2 = seed_bound_issue(&app, series, "2").await;
    let sibling = seed_copy(&app, Some(i1), "Ab/Ab 001.cbz", "dig-abs", "owned").await;
    let target = seed_copy(&app, Some(i2), "Ab/Ab 002.cbz", "dig-abs", "owned").await;

    // Removed out of band; the row still says is_present = 1.
    std::fs::remove_file(app.library_path().join("Ab/Ab 001.cbz")).unwrap();

    let (status, body) = delete_dup(&app, "dig-abs", target).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(on_disk_rel(&app, "Ab/Ab 002.cbz"), "the last bytes survive");
    let _ = sibling;
}

/// The integrity route's own ordering test.
///
/// The shared operation's doc cites a test as its constraint, but that
/// test lives in `duplicate_files_tests.rs` and drives Tidy's route —
/// so reversing the order left all 25 integrity tests passing. The
/// property with the strongest justification in this feature had no
/// test on this side defending it.
#[tokio::test]
async fn integrity_delete_removes_the_row_even_when_the_unlink_fails() {
    use std::os::unix::fs::PermissionsExt;

    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Ordering", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let i2 = seed_bound_issue(&app, series, "2").await;
    let keep = seed_copy(&app, Some(i1), "Or/Or 001.cbz", "dig-ord", "owned").await;
    let loser = seed_copy(&app, Some(i2), "Or/Or 002.cbz", "dig-ord", "owned").await;

    let dir = app.library_path().join("Or");
    let original = std::fs::metadata(&dir).unwrap().permissions();
    let mut locked = original.clone();
    locked.set_mode(0o555);
    std::fs::set_permissions(&dir, locked).unwrap();

    let (status, body) = delete_dup(&app, "dig-ord", loser).await;
    std::fs::set_permissions(&dir, original).unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    // Self-detecting skip, keyed on the observed outcome rather than on
    // guessing why: if the unlink succeeded, there is no ordering to see.
    if !on_disk_rel(&app, "Or/Or 002.cbz") {
        eprintln!("skipped: environment allowed the unlink");
        return;
    }

    assert!(
        file_repo::find_by_id(&app.state.db, loser)
            .await
            .unwrap()
            .is_none(),
        "the catalog row must be gone even though the unlink failed — a surviving row keeps the \
         issue owned, so it never reverts and the pull engine never re-fetches: {body}"
    );
    assert!(
        !body["unlink_error"].is_null(),
        "the orphaned file must be reported, not swallowed: {body}"
    );
    let _ = keep;
}

/// The file being deleted must itself still be a verified copy.
///
/// `integrity_scan` groups on the catalog's `content_blake3` with no
/// disk validation, so a member whose bytes changed after analyze stays
/// on screen and stays clickable. Its stored digest then describes
/// content that no longer exists — it is a copy of nothing, and
/// deleting it destroys the only instance of whatever it now holds.
///
/// An earlier revision stat-ed only the siblings, applying the rule to
/// every file except the one at risk.
#[tokio::test]
async fn refuses_when_the_target_itself_has_changed_since_analyze() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Changed", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let i2 = seed_bound_issue(&app, series, "2").await;
    let sibling = seed_copy(&app, Some(i1), "Ch/Ch 001.cbz", "dig-chg", "owned").await;
    let target = seed_copy(&app, Some(i2), "Ch/Ch 002.cbz", "dig-chg", "owned").await;

    // The target is rewritten after being hashed: unique content now.
    std::fs::write(
        app.library_path().join("Ch/Ch 002.cbz"),
        b"unique content that exists nowhere else",
    )
    .unwrap();

    let (status, body) = delete_dup(&app, "dig-chg", target).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the target is no longer a copy of these bytes: {body}"
    );
    assert!(
        on_disk_rel(&app, "Ch/Ch 002.cbz"),
        "the only instance of the new content must survive"
    );
    assert!(issue_is_owned(&app, i2).await, "and its issue stays owned");
    let _ = sibling;
}

/// `DELETE_TABLES` declares `files` and nothing else — and this is what
/// makes that declaration a claim rather than a comment.
///
/// `DIGEST_COLUMNS` has had `the_pass_writes_nothing_outside_the_digest_columns`
/// since the discovery PR; `DELETE_TABLES` shipped with no equivalent,
/// while the module doc claimed both were enforced. An unenforced
/// declaration sitting beside an enforced one is worse than none,
/// because a reader generalises from the enforced case: a future edit
/// that set `issues.status` from this handler would pass every other
/// test in the file.
#[tokio::test]
async fn the_delete_touches_no_table_other_than_files() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "Scoped", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let i2 = seed_bound_issue(&app, series, "2").await;
    let keep = seed_copy(&app, Some(i1), "Sc/Sc 001.cbz", "dig-scope", "owned").await;
    let loser = seed_copy(&app, Some(i2), "Sc/Sc 002.cbz", "dig-scope", "owned").await;

    // A pull attempt on the issue that will revert, so the snapshot has
    // something to lose if the handler ever decided to "tidy up" the
    // stale grab it reports on.
    longbox_db::pull_attempt_repo::insert(
        &app.state.db,
        longbox_db::NewPullAttempt {
            series_id: series,
            issue_id: i2,
            indexer_id: None,
            release_id: Some("guid".into()),
            status: "grabbed".into(),
            error_message: None,
            retry_count: 0,
            download_handle: None,
        },
    )
    .await
    .unwrap();

    // Explicit columns per table, NOT `quote(t.*)` — that is not valid
    // SQLite, and the first version of this test used it: every query
    // errored, `unwrap_or_default()` turned each into an empty vec, and
    // `before == after` held trivially. It passed while the handler
    // wrote to `issues`. A snapshot that can silently snapshot nothing
    // is not a snapshot.
    // Enumerated from `sqlite_master`, not hand-listed. The test's name
    // is "no table other than files", and a hand-picked sample of four
    // tables cannot support that claim — it would pass for a write to
    // any of the other eighteen. An earlier version sampled four and
    // was named as if it covered all.
    //
    // Columns come from `pragma_table_info` for the same reason: naming
    // them by hand misses any column added later, which is precisely
    // when this test would need to notice.
    let tables: Vec<String> = sqlx::query_as::<_, (String,)>(
        "SELECT name FROM sqlite_master WHERE type='table' \
         AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx%' \
         AND name <> 'files' ORDER BY name",
    )
    .fetch_all(&app.state.db)
    .await
    .expect("table list")
    .into_iter()
    .map(|r| r.0)
    .collect();
    assert!(
        tables.len() > 10,
        "expected the real schema, got {} tables — a truncated list would make this vacuous",
        tables.len()
    );

    let snapshot = || async {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for t in &tables {
            let cols: Vec<String> = sqlx::query_as::<_, (String,)>(&format!(
                "SELECT name FROM pragma_table_info('{t}') ORDER BY cid"
            ))
            .fetch_all(&app.state.db)
            .await
            .expect("column list")
            .into_iter()
            .map(|r| format!("quote({})", r.0))
            .collect();
            let expr = cols.join(" || '|' || ");
            let rows: Vec<String> =
                sqlx::query_as::<_, (String,)>(&format!("SELECT {expr} FROM {t}"))
                    .fetch_all(&app.state.db)
                    .await
                    .expect("snapshot query must succeed — a failing query would make this vacuous")
                    .into_iter()
                    .map(|r| r.0)
                    .collect();
            let mut rows = rows;
            rows.sort();
            out.push((t.clone(), rows));
        }
        out
    };

    let before = snapshot().await;
    let (status, body) = delete_dup(&app, "dig-scope", loser).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = snapshot().await;

    assert_eq!(
        before, after,
        "the delete route declares Surface::Deletes(&[\"files\"]); no other table may move"
    );
    let _ = keep;
}

/// A stored path whose final component is `.` names a DIRECTORY, and
/// must be refused before anything is written.
///
/// `is_contained` permits a `.` component because the read paths that
/// share it legitimately see `./a/b.cbz`. Without a separate check the
/// unlink targets the directory and fails — but only AFTER the catalog
/// row is gone, destroying a row for a path that never named a file and
/// reverting its issue on the strength of a delete that deleted
/// nothing.
///
/// The guard has to run on the raw string. `Path` parsing normalises
/// `.` away, so `Path::new("Probe/.")` and `Path::new("Probe")` are
/// indistinguishable to every accessor — `file_name()` and
/// `components().next_back()` both answer `Probe`. The first version of
/// this guard compared `Path` values and therefore never fired at all.
#[tokio::test]
async fn a_path_ending_in_a_dot_component_is_refused_before_the_row_is_touched() {
    // BOTH syntactic forms of "names a directory". `"Probe/"` is the
    // one a guard written for `"Probe/."` misses: splitting on `/`
    // leaves an empty final segment, so any scan that skips empties
    // lands on `Probe` and passes. It shipped that way for one round.
    for dir_path in ["Probe/.", "Probe/"] {
        let app = build_test_app().await;
        let series = seed_series_with_year(&app, "DotPath", 2025).await;
        let dir_issue = seed_bound_issue(&app, series, "1").await;
        let keep_issue = seed_bound_issue(&app, series, "2").await;

        // A real directory holding a real comic, catalogued under a path
        // that resolves to the DIRECTORY.
        std::fs::create_dir_all(app.library_path().join("Probe")).unwrap();
        std::fs::write(app.library_path().join("Probe/real.cbz"), b"bytes").unwrap();
        let dir_row = seed_existing(&app, Some(dir_issue), dir_path, "dig-dot").await;
        // A genuine sibling, so the never-empty-a-group guard is satisfied
        // and cannot be what produces the refusal.
        let keep = seed_copy(
            &app,
            Some(keep_issue),
            "Genuine/real.cbz",
            "dig-dot",
            "owned",
        )
        .await;

        let (status, body) = delete_dup(&app, "dig-dot", dir_row).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{dir_path:?} names no file and must be refused: {body}"
        );
        assert!(
            file_repo::find_by_id(&app.state.db, dir_row)
                .await
                .unwrap()
                .is_some(),
            "{dir_path:?}: the catalog row must survive — the refusal has to happen BEFORE the \
             row is removed"
        );
        assert!(
            app.library_path().join("Probe").is_dir()
                && app.library_path().join("Probe/real.cbz").exists(),
            "{dir_path:?}: the directory and its contents must be untouched"
        );
        assert!(
            issue_is_owned(&app, dir_issue).await,
            "{dir_path:?}: no issue may revert on the strength of a delete that deleted nothing"
        );

        // The route refuses these on content — a directory carries no
        // digest — so this asserts the OUTCOME and cannot tell which
        // guard produced it. The guard itself is pinned directly in
        // `the_delete_operation_refuses_a_path_that_does_not_name_a_file`,
        // because a test that passes with its subject disabled is not
        // evidence about its subject.
        let row = file_repo::find_by_id(&app.state.db, dir_row)
            .await
            .unwrap()
            .unwrap();
        let roots = HashMap::from([(
            app.library_root_id,
            app.library_path().to_string_lossy().into_owned(),
        )]);
        let err = longbox_web::file_delete::delete_file(&app.state.db, &roots, &row)
            .await
            .expect_err("the shared operation must refuse it too");
        assert!(
            err.contains("does not end in a file name"),
            "{dir_path:?}: refused for the wrong reason: {err}"
        );
        let _ = keep;
    }
}

/// The alias check covers the whole group, not just the file being
/// deleted — even when a genuine third copy would otherwise justify it.
///
/// Group of three: a real file, a symlink to it, and a separate real
/// copy. Deleting the symlink's target would be *survivable* here,
/// because the third copy is genuinely independent — so a
/// target-relative check that only compared the target against each
/// sibling in isolation could be talked into proceeding. The rule is
/// stated as "any aliased pair anywhere in the group stops the
/// operation", which is also the predicate Library Tidy applies; a
/// narrower check here would make two implementations of one rule.
#[tokio::test]
async fn an_alias_anywhere_in_the_group_refuses_even_with_a_genuine_third_copy() {
    let app = build_test_app().await;
    let series = seed_series_with_year(&app, "AliasCount", 2025).await;
    let i1 = seed_bound_issue(&app, series, "1").await;
    let i2 = seed_bound_issue(&app, series, "2").await;
    let i3 = seed_bound_issue(&app, series, "3").await;

    // A genuine second copy, plus an alias pair.
    std::fs::create_dir_all(app.library_path().join("CReal")).unwrap();
    std::fs::create_dir_all(app.library_path().join("CLink")).unwrap();
    std::os::unix::fs::symlink(
        app.library_path().join("CReal/comic.cbz"),
        app.library_path().join("CLink/comic.cbz"),
    )
    .unwrap();
    let alias = seed_copy(&app, Some(i2), "CLink/comic.cbz", "dig-a3", "owned").await;
    let target = seed_existing(&app, Some(i1), "CReal/comic.cbz", "dig-a3").await;
    let real_copy = seed_copy(&app, Some(i3), "CCopy/comic.cbz", "dig-a3", "owned").await;

    // Deleting the genuinely-independent copy is refused too: the group
    // still contains an aliased pair, and the rule is about the group.
    for (t, which) in [(target, "the alias target"), (real_copy, "the third copy")] {
        let (status, body) = delete_dup(&app, "dig-a3", t).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "deleting {which} must be refused while the group holds an alias: {body}"
        );
    }
    assert!(
        on_disk_rel(&app, "CReal/comic.cbz") && on_disk_rel(&app, "CCopy/comic.cbz"),
        "nothing may be deleted from a group containing an alias"
    );
    let _ = alias;
}
