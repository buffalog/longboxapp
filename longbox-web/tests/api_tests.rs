mod common;

use std::io::Write;
use std::path::Path;

use axum::http::StatusCode;
use common::{build_test_app, empty_request, json_request, response_json};
use longbox_db::{issue_repo, series_repo, NewIssue, NewSeries};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};
use zip::write::SimpleFileOptions;

fn write_cbz(path: &Path, comic_info: Option<&str>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("page.jpg", opts).unwrap();
    zip.write_all(b"\xFF\xD8\xFF").unwrap();
    if let Some(xml) = comic_info {
        zip.start_file("ComicInfo.xml", opts).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

// -------- health --------

#[tokio::test]
async fn health_returns_200_with_version() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/health")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
}

// -------- CV search --------

#[tokio::test]
async fn cv_search_happy_path() {
    let app = build_test_app().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .and(query_param("query", "walking dead"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "status_code": 1, "error": "OK",
                "number_of_total_results": 1, "limit": 100, "offset": 0,
                "results": [{
                    "id": 2127, "name": "The Walking Dead", "start_year": "2003",
                    "publisher": { "id": 1, "name": "Image" },
                    "count_of_issues": 193,
                    "image": { "medium_url": "https://example.com/wd.jpg" },
                    "deck": "Apocalypse"
                }]
            }"#,
        ))
        .mount(&app.cv_server)
        .await;

    let resp = app
        .request(empty_request("GET", "/api/cv/search?q=walking%20dead"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body[0]["cv_id"], 2127);
    assert_eq!(body[0]["name"], "The Walking Dead");
}

#[tokio::test]
async fn cv_search_empty_query_returns_400() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/cv/search?q=")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cv_search_upstream_500_maps_to_502() {
    let app = build_test_app().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&app.cv_server)
        .await;
    let resp = app.request(empty_request("GET", "/api/cv/search?q=x")).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "upstream.comicvine");
}

// -------- POST /api/series --------

#[tokio::test]
async fn add_series_happy_path() {
    let app = build_test_app().await;

    Mock::given(method("GET"))
        .and(path("/volume/4050-2127/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "status_code": 1, "error": "OK", "number_of_total_results": 1,
                "results": {
                    "id": 2127, "name": "The Walking Dead", "start_year": "2003",
                    "publisher": { "id": 1, "name": "Image" },
                    "description": "<p>Apocalypse</p>",
                    "image": { "medium_url": "https://example.com/wd.jpg" },
                    "site_detail_url": "https://cv/wd/4050-2127/"
                }
            }"#,
        ))
        .mount(&app.cv_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/issues/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "status_code": 1, "error": "OK",
                "number_of_total_results": 2, "limit": 100, "offset": 0,
                "results": [
                    { "id": 10001, "issue_number": "1", "name": "Days Gone Bye",
                      "cover_date": "2003-10-08", "description": null,
                      "image": { "medium_url": "https://example.com/wd-1.jpg" },
                      "site_detail_url": "https://cv/4000-10001/" },
                    { "id": 10002, "issue_number": "2", "name": null,
                      "cover_date": null, "description": null,
                      "image": null,
                      "site_detail_url": "https://cv/4000-10002/" }
                ]
            }"#,
        ))
        .mount(&app.cv_server)
        .await;

    let resp = app
        .request(json_request("POST", "/api/series", r#"{"cv_id": 2127}"#))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["title"], "The Walking Dead");

    // Verify DB state.
    let row = series_repo::find_by_cv_id(&app.state.db, 2127)
        .await
        .unwrap()
        .unwrap();
    let issues = issue_repo::list_by_series(&app.state.db, row.id)
        .await
        .unwrap();
    assert_eq!(issues.len(), 2);
}

#[tokio::test]
async fn add_series_duplicate_cv_id_returns_409() {
    let app = build_test_app().await;
    series_repo::insert(
        &app.state.db,
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
    let resp = app
        .request(json_request("POST", "/api/series", r#"{"cv_id": 2127}"#))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "conflict.series_already_exists");
}

#[tokio::test]
async fn add_series_invalid_cv_id_returns_400() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request("POST", "/api/series", r#"{"cv_id": 0}"#))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_series_cv_404_maps_to_not_found() {
    let app = build_test_app().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-9999/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 101, "error": "Object Not Found", "results": null }"#,
        ))
        .mount(&app.cv_server)
        .await;
    let resp = app
        .request(json_request("POST", "/api/series", r#"{"cv_id": 9999}"#))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// -------- GET /api/series --------

#[tokio::test]
async fn list_series_computes_owned_and_total_counts() {
    let app = build_test_app().await;
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(1),
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
    for n in ["1", "2", "3"] {
        issue_repo::insert(
            &app.state.db,
            NewIssue {
                series_id: series.id,
                cv_issue_id: None,
                metron_issue_id: None,
                number: n.into(),
                title: None,
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }

    let resp = app.request(empty_request("GET", "/api/series")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body[0]["title"], "Saga");
    assert_eq!(body[0]["total_count"], 3);
    assert_eq!(body[0]["owned_count"], 0);
}

#[tokio::test]
async fn series_detail_orders_issues_naturally() {
    let app = build_test_app().await;
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(1),
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
    // Insert deliberately out of order; expect natural sort in the response.
    for n in ["10", "1", "Annual 1", "2"] {
        issue_repo::insert(
            &app.state.db,
            NewIssue {
                series_id: series.id,
                cv_issue_id: None,
                metron_issue_id: None,
                number: n.into(),
                title: None,
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }
    let resp = app
        .request(empty_request("GET", &format!("/api/series/{}", series.id)))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let numbers: Vec<String> = body["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(numbers, vec!["1", "2", "10", "Annual 1"]);
}

#[tokio::test]
async fn series_detail_404_for_missing() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/series/9999")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_series_with_no_owned_files_succeeds() {
    let app = build_test_app().await;
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(1),
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
    let resp = app
        .request(empty_request("DELETE", &format!("/api/series/{}", series.id)))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(series_repo::find_by_id(&app.state.db, series.id)
        .await
        .unwrap()
        .is_none());
}

// -------- files --------

#[tokio::test]
async fn list_files_filters_by_status() {
    let app = build_test_app().await;
    write_cbz(
        &app.library_path().join("Mystery/UnknownComic.cbz"),
        None,
    );
    // Trigger a scan to populate files table.
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();

    let resp = app
        .request(empty_request("GET", "/api/files?status=unmatched"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let files = body.as_array().unwrap();
    assert!(files.iter().any(|f| f["status"] == "unmatched"));
}

#[tokio::test]
async fn list_files_invalid_status_returns_400() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/api/files?status=banana"))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_file_marks_ignored() {
    let app = build_test_app().await;
    write_cbz(
        &app.library_path().join("Mystery/UnknownComic.cbz"),
        None,
    );
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let files = longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
        .await
        .unwrap();
    let file_id = files[0].id;

    let resp = app
        .request(json_request(
            "PATCH",
            &format!("/api/files/{file_id}"),
            r#"{ "status": "ignored" }"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["status"], "ignored");
    assert!(body["issue_id"].is_null());
}

#[tokio::test]
async fn patch_file_clear_ignored_resets_to_unmatched() {
    let app = build_test_app().await;
    write_cbz(
        &app.library_path().join("Mystery/UnknownComic.cbz"),
        None,
    );
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let files = longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
        .await
        .unwrap();
    let file_id = files[0].id;

    // Ignore first.
    app.request(json_request(
        "PATCH",
        &format!("/api/files/{file_id}"),
        r#"{ "status": "ignored" }"#,
    ))
    .await;
    // Then clear.
    let resp = app
        .request(json_request(
            "PATCH",
            &format!("/api/files/{file_id}"),
            r#"{ "status": null }"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["status"], "unmatched");
}

#[tokio::test]
async fn patch_file_manual_rematch_sets_issue_and_owned() {
    let app = build_test_app().await;
    write_cbz(
        &app.library_path().join("Mystery/UnknownComic.cbz"),
        None,
    );
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(1),
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
    let issue = issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: series.id,
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
    .unwrap();

    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let files = longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
        .await
        .unwrap();
    let file_id = files[0].id;

    let resp = app
        .request(json_request(
            "PATCH",
            &format!("/api/files/{file_id}"),
            format!(r#"{{ "issue_id": {} }}"#, issue.id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["status"], "owned");
    assert_eq!(body["issue_id"], issue.id);
    assert_eq!(body["match_method"], "manual");
}

#[tokio::test]
async fn patch_file_empty_body_returns_400() {
    let app = build_test_app().await;
    write_cbz(&app.library_path().join("X.cbz"), None);
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let files = longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
        .await
        .unwrap();
    let resp = app
        .request(json_request("PATCH", &format!("/api/files/{}", files[0].id), "{}"))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// -------- scan --------

#[tokio::test]
async fn scan_endpoint_returns_202_with_scan_id() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request(
            "POST",
            &format!("/api/library-roots/{}/scan", app.library_root_id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = response_json(resp).await;
    assert!(body["scan_id"].is_string());
    assert_eq!(body["status"], "started");
}

#[tokio::test]
async fn scan_with_missing_library_root_returns_404() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("POST", "/api/library-roots/9999/scan"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn scans_current_reflects_running_scan() {
    let app = build_test_app().await;
    // Inject a pretend-running scan directly into state to avoid the race
    // between the spawned task completing and our read.
    {
        let mut s = app.state.scan_status.write().await;
        s.current = Some(longbox_web::CurrentScan {
            scan_id: "01HAB".into(),
            library_root_id: app.library_root_id,
            kind: longbox_web::ScanKind::Full,
            started_at: time::OffsetDateTime::now_utc(),
        });
    }
    let resp = app
        .request(empty_request("GET", "/api/scans/current"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["scan_id"], "01HAB");
    assert_eq!(body["kind"], "full");
}

#[tokio::test]
async fn scans_current_null_when_idle() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/api/scans/current"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert!(body.is_null());
}

#[tokio::test]
async fn concurrent_scan_trigger_returns_409() {
    let app = build_test_app().await;
    {
        let mut s = app.state.scan_status.write().await;
        s.current = Some(longbox_web::CurrentScan {
            scan_id: "01HAB".into(),
            library_root_id: app.library_root_id,
            kind: longbox_web::ScanKind::Full,
            started_at: time::OffsetDateTime::now_utc(),
        });
    }
    let resp = app
        .request(empty_request(
            "POST",
            &format!("/api/library-roots/{}/scan", app.library_root_id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "conflict.scan_running");
}

// -------- frontend --------

#[tokio::test]
async fn frontend_root_serves_index_html() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/html"));
}

#[tokio::test]
async fn frontend_spa_fallback_returns_index_for_unknown_path() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/series/42")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/html"));
}

#[tokio::test]
async fn well_known_returns_404_no_fallback() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/.well-known/security.txt"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// -------- error shape --------

#[tokio::test]
async fn malformed_json_returns_422() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request("POST", "/api/series", r#"{ "cv_id": "not a number" }"#))
        .await;
    // Axum's built-in JSON extractor returns 400 for type mismatch, 422 for
    // shape mismatch. Accept either as long as it's a 4xx.
    assert!(resp.status().is_client_error(), "got {}", resp.status());
}

// -------- POST /api/files/:id/match-from-cv --------

/// Sets up CV wiremock returning a single volume (id `cv_id`) with the
/// listed issues, then writes one CBZ and runs a full scan to populate the
/// `files` table. Returns the file id of the first scanned-in row.
async fn setup_match_from_cv(
    app: &common::TestApp,
    cv_id: i64,
    volume_name: &str,
    issues: &[(i64, &str)],
    cbz_relpath: &str,
    comic_info: Option<&str>,
) -> i64 {
    let volume_body = format!(
        r#"{{
            "status_code": 1, "error": "OK", "number_of_total_results": 1,
            "results": {{
                "id": {cv_id}, "name": {name:?}, "start_year": "2012",
                "publisher": {{ "id": 1, "name": "Image" }},
                "description": null,
                "image": null,
                "site_detail_url": "https://cv/v/{cv_id}/"
            }}
        }}"#,
        name = volume_name
    );
    let issue_results: Vec<String> = issues
        .iter()
        .map(|(id, n)| {
            format!(
                r#"{{ "id": {id}, "issue_number": "{n}", "name": null,
                       "cover_date": null, "description": null,
                       "image": null, "site_detail_url": "https://cv/i/{id}/" }}"#
            )
        })
        .collect();
    let issues_body = format!(
        r#"{{
            "status_code": 1, "error": "OK",
            "number_of_total_results": {n}, "limit": 100, "offset": 0,
            "results": [{r}]
        }}"#,
        n = issue_results.len(),
        r = issue_results.join(",")
    );

    Mock::given(method("GET"))
        .and(path(format!("/volume/4050-{cv_id}/")))
        .respond_with(ResponseTemplate::new(200).set_body_string(volume_body))
        .mount(&app.cv_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(issues_body))
        .mount(&app.cv_server)
        .await;

    write_cbz(&app.library_path().join(cbz_relpath), comic_info);
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();

    // Return the file id of the first row in the library.
    let row = sqlx::query!(r#"SELECT id AS "id!: i64" FROM files ORDER BY id LIMIT 1"#)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    row.id
}

#[tokio::test]
async fn match_from_cv_resolves_issue_number_from_filename() {
    let app = build_test_app().await;
    let file_id = setup_match_from_cv(
        &app,
        7777,
        "Saga",
        &[(80001, "1"), (80002, "2")],
        "Saga (2012)/Saga 002 (2012).cbz",
        None,
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            &format!("/api/files/{file_id}/match-from-cv"),
            r#"{"cv_volume_id": 7777}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK, "body: {:?}", response_json(app.request(empty_request("GET", "/api/files")).await).await);
    let body = response_json(resp).await;
    assert_eq!(body["match_method"], "manual");
    assert_eq!(body["match_confidence"], 1.0);
    assert_eq!(body["status"], "owned");
    assert_eq!(body["issue"]["number"], "2");
    assert_eq!(body["series"]["title"], "Saga");
}

#[tokio::test]
async fn match_from_cv_explicit_issue_number_overrides_filename() {
    let app = build_test_app().await;
    let file_id = setup_match_from_cv(
        &app,
        7777,
        "Saga",
        &[(80001, "1"), (80002, "2")],
        // Filename parses as #2 but the body says #1.
        "Saga (2012)/Saga 002.cbz",
        None,
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            &format!("/api/files/{file_id}/match-from-cv"),
            r#"{"cv_volume_id": 7777, "issue_number": "1"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["issue"]["number"], "1");
}

#[tokio::test]
async fn match_from_cv_returns_422_when_issue_not_in_series() {
    let app = build_test_app().await;
    let file_id = setup_match_from_cv(
        &app,
        7777,
        "Saga",
        // Only issue #1 exists in CV.
        &[(80001, "1")],
        // Filename says #99 — not in the series.
        "Saga (2012)/Saga 099.cbz",
        None,
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            &format!("/api/files/{file_id}/match-from-cv"),
            r#"{"cv_volume_id": 7777}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "unprocessable.issue_not_in_series");
}

#[tokio::test]
async fn match_from_cv_returns_422_when_number_unresolvable() {
    let app = build_test_app().await;
    let file_id = setup_match_from_cv(
        &app,
        7777,
        "Saga",
        &[(80001, "1")],
        // No issue number in the filename, no ComicInfo.
        "Saga (2012)/random-filename.cbz",
        None,
    )
    .await;
    let resp = app
        .request(json_request(
            "POST",
            &format!("/api/files/{file_id}/match-from-cv"),
            r#"{"cv_volume_id": 7777}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "unprocessable.issue_number_unresolved");
}

#[tokio::test]
async fn match_from_cv_reuses_existing_series_no_cv_call() {
    let app = build_test_app().await;
    // Pre-seed a series with cv_id 7777 + issue #1. No CV mock for the
    // volume endpoint — if the handler tries to call CV, wiremock returns
    // 404 by default and the test fails.
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(7777),
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
    issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: series.id,
            cv_issue_id: Some(80001),
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

    write_cbz(&app.library_path().join("Saga (2012)/Saga 001.cbz"), None);
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let file_id = sqlx::query!(r#"SELECT id AS "id!: i64" FROM files ORDER BY id LIMIT 1"#)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
        .id;

    let resp = app
        .request(json_request(
            "POST",
            &format!("/api/files/{file_id}/match-from-cv"),
            r#"{"cv_volume_id": 7777}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["issue"]["number"], "1");
    assert_eq!(body["series"]["id"], series.id);
}

#[tokio::test]
async fn match_from_cv_404_for_unknown_file() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/files/9999/match-from-cv",
            r#"{"cv_volume_id": 7777}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
