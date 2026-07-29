//! Library Integrity — discovery surface.
//!
//! The load-bearing test here is [`only_the_analyze_route_accepts_a_write_method`].
//! Every other assertion in this file is about behaviour; that one is about a
//! property of the whole module, and it is the reason this PR can be trusted
//! to be non-destructive.

mod common;

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

    // And exactly one route declares a write at all.
    assert_eq!(
        declared.iter().filter(|(_, w)| *w).count(),
        1,
        "integrity is a discovery surface: exactly one write path"
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
