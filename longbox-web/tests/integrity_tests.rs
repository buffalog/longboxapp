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

/// Every path the integrity module serves. Listed explicitly rather than
/// derived from the router, because axum does not expose its route table.
/// `every_listed_path_actually_exists` catches a stale entry here; the
/// `Surface` declaration in the module catches a route added without
/// declaring what it writes.
const INTEGRITY_PATHS: &[&str] = &[
    "/api/library/integrity/analyze",
    "/api/library/integrity/analyze/status",
];

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
/// Not "read-only" — the module has one write path and says so. This asserts
/// the precise property: of every (path × write-method) pair the module
/// serves, exactly ONE is accepted, and it is `POST …/analyze`.
///
/// Behavioural on purpose. A grep over query text — the obvious way to check
/// "does this module write?" — is defeated by a multiline string, a helper
/// indirection, or a macro. Sending the actual methods at the actual router
/// is not.
#[tokio::test]
async fn only_the_analyze_route_accepts_a_write_method() {
    let app = build_test_app().await;
    let mut accepted = Vec::new();

    for path in INTEGRITY_PATHS {
        for method in WRITE_METHODS {
            let resp = app.request(request(method.clone(), path)).await;
            // 405 = the path exists but refuses this method. Anything else
            // means the method got through to a handler.
            if resp.status() != StatusCode::METHOD_NOT_ALLOWED {
                accepted.push((method.clone(), *path, resp.status()));
            }
        }
    }

    assert_eq!(
        accepted.len(),
        1,
        "exactly one write path allowed; got {accepted:?}"
    );
    assert_eq!(accepted[0].0, Method::POST);
    assert_eq!(accepted[0].1, "/api/library/integrity/analyze");
}

/// A path listed above but not actually routed would make the write-surface
/// test vacuous for that path — it would 404 rather than 405 and be counted
/// as "accepted", or silently pass. This keeps the list honest.
#[tokio::test]
async fn every_listed_path_actually_exists() {
    let app = build_test_app().await;
    for path in INTEGRITY_PATHS {
        let resp = app.request(empty_request("GET", path)).await;
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{path} is listed but not routed — the write-surface test would silently skip it"
        );
    }
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
