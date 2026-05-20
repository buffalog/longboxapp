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
    // Task 4 wraps the response. Seeded blocklist doesn't include Image,
    // so Image-published Walking Dead survives.
    assert_eq!(body["results"][0]["cv_id"], 2127);
    assert_eq!(body["results"][0]["name"], "The Walking Dead");
    assert_eq!(body["filtered_count"], 0);
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
    let resp = app
        .request(empty_request("GET", "/api/cv/search?q=x"))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "upstream.comicvine");
}

#[tokio::test]
async fn cv_search_blocks_seeded_reprint_publishers() {
    let app = build_test_app().await;
    // Three "Batman" hits: DC original, a Panini reprint (default-
    // blocked), and a custom publisher we'll add to the blocklist.
    Mock::given(method("GET"))
        .and(path("/search/"))
        .and(query_param("query", "batman"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "status_code": 1, "error": "OK",
                "number_of_total_results": 3, "limit": 100, "offset": 0,
                "results": [
                    { "id": 1, "name": "Batman", "start_year": "2024",
                      "publisher": { "id": 10, "name": "DC Comics" },
                      "count_of_issues": 20,
                      "image": null, "deck": null },
                    { "id": 2, "name": "Batman", "start_year": "2024",
                      "publisher": { "id": 11, "name": "Panini Comics" },
                      "count_of_issues": 5,
                      "image": null, "deck": null },
                    { "id": 3, "name": "Batman", "start_year": "2024",
                      "publisher": { "id": 12, "name": "Custom Reprint Co" },
                      "count_of_issues": 5,
                      "image": null, "deck": null }
                ]
            }"#,
        ))
        .mount(&app.cv_server)
        .await;

    // Add a custom blocklist entry on top of the seeded defaults.
    let resp = app
        .request(json_request(
            "POST",
            "/api/publishers/filters",
            r#"{"publisher_name": "Custom Reprint Co"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .request(empty_request("GET", "/api/cv/search?q=batman"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["publisher"], "DC Comics");
    assert_eq!(body["filtered_count"], 2);
}

#[tokio::test]
async fn cv_search_show_filtered_bypasses_blocklist() {
    let app = build_test_app().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "status_code": 1, "error": "OK",
                "number_of_total_results": 2, "limit": 100, "offset": 0,
                "results": [
                    { "id": 1, "name": "Batman", "start_year": "2024",
                      "publisher": { "id": 10, "name": "DC Comics" },
                      "count_of_issues": 1, "image": null, "deck": null },
                    { "id": 2, "name": "Batman", "start_year": "2024",
                      "publisher": { "id": 11, "name": "Panini Comics" },
                      "count_of_issues": 1, "image": null, "deck": null }
                ]
            }"#,
        ))
        .mount(&app.cv_server)
        .await;
    let resp = app
        .request(empty_request(
            "GET",
            "/api/cv/search?q=batman&show_filtered=true",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["results"].as_array().unwrap().len(), 2);
    assert_eq!(body["filtered_count"], 0);
}

// -------- /api/publishers/filters --------

#[tokio::test]
async fn publisher_filters_list_returns_seeded_defaults() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/api/publishers/filters"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["publisher_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Panini Comics"));
    assert!(names.contains(&"Éditions Glénat"));
    assert!(names.contains(&"Arnoldo Mondadori Editore"));
}

#[tokio::test]
async fn publisher_filters_create_then_delete() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/publishers/filters",
            r#"{"publisher_name": "TestCo Reprints"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let id = body["id"].as_i64().unwrap();
    assert_eq!(body["publisher_name"], "TestCo Reprints");
    assert_eq!(body["mode"], "block");

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/publishers/filters/{id}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/publishers/filters/{id}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn publisher_filters_create_case_insensitive_conflict() {
    let app = build_test_app().await;
    // "panini comics" (lowercase) collides with the seeded "Panini Comics".
    let resp = app
        .request(json_request(
            "POST",
            "/api/publishers/filters",
            r#"{"publisher_name": "panini comics"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "conflict.publisher_filter_exists");
}

#[tokio::test]
async fn publisher_filters_create_empty_returns_400() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/publishers/filters",
            r#"{"publisher_name": "   "}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn publisher_filters_reset_reinserts_missing_defaults() {
    let app = build_test_app().await;
    // Delete one of the seeded defaults.
    let list = response_json(
        app.request(empty_request("GET", "/api/publishers/filters"))
            .await,
    )
    .await;
    let target = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["publisher_name"] == "Salvat")
        .unwrap();
    let id = target["id"].as_i64().unwrap();
    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/publishers/filters/{id}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .request(empty_request(
            "POST",
            "/api/publishers/filters/reset-defaults",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["inserted"], 1);

    // After reset, Salvat is back.
    let list = response_json(
        app.request(empty_request("GET", "/api/publishers/filters"))
            .await,
    )
    .await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["publisher_name"] == "Salvat"));
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
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{}", series.id),
        ))
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
    write_cbz(&app.library_path().join("Mystery/UnknownComic.cbz"), None);
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
    write_cbz(&app.library_path().join("Mystery/UnknownComic.cbz"), None);
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
    write_cbz(&app.library_path().join("Mystery/UnknownComic.cbz"), None);
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
    write_cbz(&app.library_path().join("Mystery/UnknownComic.cbz"), None);
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
        .request(json_request(
            "PATCH",
            &format!("/api/files/{}", files[0].id),
            "{}",
        ))
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
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/html"));
}

#[tokio::test]
async fn frontend_spa_fallback_returns_index_for_unknown_path() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/series/42")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
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
        .request(json_request(
            "POST",
            "/api/series",
            r#"{ "cv_id": "not a number" }"#,
        ))
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
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "body: {:?}",
        response_json(app.request(empty_request("GET", "/api/files")).await).await
    );
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
    assert_eq!(
        body["error"]["code"],
        "unprocessable.issue_number_unresolved"
    );
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

// -------- GET /api/missing --------

#[tokio::test]
async fn missing_empty_on_fresh_catalog() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/missing")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["missing"].as_array().unwrap().len(), 0);
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn missing_lists_issues_with_no_owned_file_and_natural_sorts() {
    let app = build_test_app().await;
    // Two series. Saga has issues 1, 2, 10, "Annual 1"; 1 is owned, the
    // rest missing. Wolverine has issue 1 missing.
    let saga = series_repo::insert(
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
    let wolverine = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(2),
            metron_id: None,
            title: "Wolverine".into(),
            sort_title: "wolverine".into(),
            start_year: Some(1982),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    let mut saga_issue_ids = Vec::new();
    for n in ["1", "2", "10", "Annual 1"] {
        let row = issue_repo::insert(
            &app.state.db,
            NewIssue {
                series_id: saga.id,
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
        saga_issue_ids.push(row.id);
    }
    issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: wolverine.id,
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

    // Mark Saga #1 as owned via a real scan: write a CBZ matched by
    // filename, then PATCH it onto the issue.
    write_cbz(&app.library_path().join("Saga (2012)/Saga 001.cbz"), None);
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let file_id = sqlx::query!(
        r#"SELECT id AS "id!: i64" FROM files
           WHERE path_relative = 'Saga (2012)/Saga 001.cbz'"#
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap()
    .id;
    app.request(json_request(
        "PATCH",
        &format!("/api/files/{file_id}"),
        &format!(r#"{{"issue_id": {}}}"#, saga_issue_ids[0]),
    ))
    .await;

    // Unfiltered: 3 missing in Saga + 1 missing in Wolverine. Default
    // sort = series. Saga before Wolverine (sort_title), and issues
    // within Saga in natural order: 2, 10, Annual 1.
    let resp = app.request(empty_request("GET", "/api/missing")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["total"], 4);
    let numbers: Vec<&str> = body["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["number"].as_str().unwrap())
        .collect();
    assert_eq!(numbers, vec!["2", "10", "Annual 1", "1"]);
    let series_titles: Vec<&str> = body["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["series"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(series_titles, vec!["Saga", "Saga", "Saga", "Wolverine"]);
}

#[tokio::test]
async fn missing_filters_by_series_id() {
    let app = build_test_app().await;
    let saga = series_repo::insert(
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
    let wolverine = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(2),
            metron_id: None,
            title: "Wolverine".into(),
            sort_title: "wolverine".into(),
            start_year: Some(1982),
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
            series_id: saga.id,
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
    issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: wolverine.id,
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

    let resp = app
        .request(empty_request(
            "GET",
            &format!("/api/missing?series_id={}", saga.id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["missing"][0]["series"]["title"], "Saga");
}

#[tokio::test]
async fn missing_sorts_by_cover_date_when_requested() {
    let app = build_test_app().await;
    let s = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(1),
            metron_id: None,
            title: "Test".into(),
            sort_title: "test".into(),
            start_year: Some(2024),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    for (n, d) in [
        ("3", "2024-03-01"),
        ("1", "2024-01-01"),
        ("2", "2024-02-01"),
    ] {
        issue_repo::insert(
            &app.state.db,
            NewIssue {
                series_id: s.id,
                cv_issue_id: None,
                metron_issue_id: None,
                number: n.into(),
                title: None,
                cover_date: Some(d.into()),
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }
    let resp = app
        .request(empty_request("GET", "/api/missing?sort=cover_date"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let dates: Vec<&str> = body["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["cover_date"].as_str().unwrap())
        .collect();
    assert_eq!(dates, vec!["2024-01-01", "2024-02-01", "2024-03-01"]);
}

// -------- GET /api/dashboard/activity --------

#[tokio::test]
async fn dashboard_activity_empty_on_fresh_catalog() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/api/dashboard/activity"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["recent_series"].as_array().unwrap().len(), 0);
    assert_eq!(body["recent_matches"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn dashboard_activity_lists_recent_series_and_matches() {
    let app = build_test_app().await;
    // Seed a series + issue, then create a file matched to it.
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(101),
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
    .unwrap();
    let issue = issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: series.id,
            cv_issue_id: Some(8001),
            metron_issue_id: None,
            number: "1".into(),
            title: Some("Saga #1".into()),
            cover_date: None,
            summary: None,
            cover_url: Some("https://example.com/saga-1.jpg".into()),
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
    let file_id = sqlx::query!(
        r#"SELECT id AS "id!: i64" FROM files
           WHERE path_relative = 'Saga (2012)/Saga 001.cbz'"#
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap()
    .id;
    // Manually flip it to matched via the PATCH path — that exercises
    // the matched_at update rule end-to-end.
    let resp = app
        .request(json_request(
            "PATCH",
            &format!("/api/files/{file_id}"),
            &format!(r#"{{"issue_id": {}}}"#, issue.id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .request(empty_request("GET", "/api/dashboard/activity?limit=6"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;

    let series_list = body["recent_series"].as_array().unwrap();
    assert_eq!(series_list.len(), 1);
    assert_eq!(series_list[0]["title"], "Saga");
    assert_eq!(series_list[0]["owned_count"], 1);
    assert_eq!(series_list[0]["total_count"], 1);

    let match_list = body["recent_matches"].as_array().unwrap();
    assert_eq!(match_list.len(), 1);
    assert_eq!(match_list[0]["path_relative"], "Saga (2012)/Saga 001.cbz");
    assert_eq!(match_list[0]["issue"]["number"], "1");
    assert_eq!(match_list[0]["issue"]["title"], "Saga #1");
    assert_eq!(match_list[0]["series"]["title"], "Saga");
    assert!(match_list[0]["matched_at"].as_str().is_some());
}

#[tokio::test]
async fn dashboard_activity_validates_limit() {
    let app = build_test_app().await;
    for bad in ["0", "51", "9999"] {
        let resp = app
            .request(empty_request(
                "GET",
                &format!("/api/dashboard/activity?limit={bad}"),
            ))
            .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "limit={bad}");
    }
}

// -------- GET /api/settings --------

#[tokio::test]
async fn settings_returns_configured_values() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/settings")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    // Mirrors the test harness in common/mod.rs.
    assert_eq!(body["match_threshold"], 0.85);
    assert_eq!(body["log_level"], "info");
    assert_eq!(body["bind_address"], "0.0.0.0:0");
    assert_eq!(body["database_url"], "sqlite::memory:");
    assert!(body["library_root_path"].as_str().unwrap().contains("/"));
    // `comicvine_api_key_configured` is structurally always true today
    // (boot fails without a key); shape contract only.
    assert_eq!(body["comicvine_api_key_configured"], true);
    // Bare value sanity, not a literal version assertion.
    assert!(body["version"].as_str().unwrap().starts_with("0."));
}

#[tokio::test]
async fn settings_never_exposes_the_cv_api_key() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/settings")).await;
    let body = response_json(resp).await;
    let raw = serde_json::to_string(&body).unwrap();
    // The test harness sets the key to "test-key" — assert no string in
    // the response body contains it. Belt-and-suspenders against an
    // accidental future field rename that leaks the value.
    assert!(
        !raw.contains("test-key"),
        "response leaked the CV API key: {raw}"
    );
}

// -------- POST /api/files/match-folder-from-cv --------

/// Mounts wiremock for a single CV volume with the given issues, then
/// writes the given CBZs (each as `(relpath, comic_info_xml?)`) into the
/// library and runs a full scan to populate `files`.
async fn setup_folder_match(
    app: &common::TestApp,
    cv_id: i64,
    volume_name: &str,
    issues: &[(i64, &str)],
    files: &[(&str, Option<&str>)],
) {
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

    for (relpath, comic_info) in files {
        write_cbz(&app.library_path().join(relpath), *comic_info);
    }
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn match_folder_happy_path_matches_all_resolvable() {
    let app = build_test_app().await;
    setup_folder_match(
        &app,
        9001,
        "Saga",
        &[(91, "1"), (92, "2"), (93, "3")],
        &[
            ("Saga (2012)/Saga 001.cbz", None),
            ("Saga (2012)/Saga 002.cbz", None),
            ("Saga (2012)/Saga 003.cbz", None),
        ],
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/files/match-folder-from-cv",
            r#"{"directory": "Saga (2012)", "cv_volume_id": 9001}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["matched_count"], 3);
    assert_eq!(body["skipped"].as_array().unwrap().len(), 0);

    // Sibling files in a different folder must not be touched.
    write_cbz(&app.library_path().join("Other Series/Other 001.cbz"), None);
}

#[tokio::test]
async fn match_folder_skips_unresolvable_and_out_of_series() {
    let app = build_test_app().await;
    // Only #1 and #2 exist in the CV volume. Folder has four files:
    //  - 001 → matches
    //  - 002 → matches
    //  - 099 → resolvable but not in series (skip: issue_not_in_series)
    //  - mystery → no parseable number (skip: issue_number_unresolved)
    setup_folder_match(
        &app,
        9002,
        "Saga",
        &[(91, "1"), (92, "2")],
        &[
            ("Saga (2012)/Saga 001.cbz", None),
            ("Saga (2012)/Saga 002.cbz", None),
            ("Saga (2012)/Saga 099.cbz", None),
            ("Saga (2012)/mystery.cbz", None),
        ],
    )
    .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/files/match-folder-from-cv",
            r#"{"directory": "Saga (2012)", "cv_volume_id": 9002}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["matched_count"], 2);
    let skipped = body["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    let reasons: Vec<&str> = skipped
        .iter()
        .map(|s| s["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"issue_not_in_series"));
    assert!(reasons.contains(&"issue_number_unresolved"));
}

#[tokio::test]
async fn match_folder_does_not_touch_owned_files() {
    let app = build_test_app().await;
    setup_folder_match(
        &app,
        9003,
        "Saga",
        &[(91, "1"), (92, "2")],
        &[
            ("Saga (2012)/Saga 001.cbz", None),
            ("Saga (2012)/Saga 002.cbz", None),
        ],
    )
    .await;

    // Manually mark #1 owned by an unrelated issue first.
    let pre_series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(8888),
            metron_id: None,
            title: "Decoy".into(),
            sort_title: "decoy".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    let pre_issue = issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: pre_series.id,
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
    let file_001 = sqlx::query!(
        r#"SELECT id AS "id!: i64" FROM files
           WHERE path_relative = 'Saga (2012)/Saga 001.cbz'"#
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap()
    .id;
    let resp = app
        .request(json_request(
            "PATCH",
            &format!("/api/files/{file_001}"),
            &format!(r#"{{"issue_id": {}}}"#, pre_issue.id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Folder match should now leave #1 alone (status=owned, not in
    // unmatched/needs_review) and match only #2.
    let resp = app
        .request(json_request(
            "POST",
            "/api/files/match-folder-from-cv",
            r#"{"directory": "Saga (2012)", "cv_volume_id": 9003}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["matched_count"], 1);
    assert_eq!(body["skipped"].as_array().unwrap().len(), 0);

    // #1 still points at the Decoy issue.
    let row = sqlx::query!(
        r#"SELECT issue_id AS "issue_id?: i64" FROM files WHERE id = ?"#,
        file_001
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(row.issue_id, Some(pre_issue.id));
}

#[tokio::test]
async fn match_folder_400_for_empty_directory() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/files/match-folder-from-cv",
            r#"{"directory": "  ", "cv_volume_id": 9001}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn match_folder_400_for_invalid_cv_volume_id() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/files/match-folder-from-cv",
            r#"{"directory": "Saga", "cv_volume_id": 0}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn match_folder_excludes_nested_subdirectories() {
    let app = build_test_app().await;
    setup_folder_match(
        &app,
        9005,
        "Saga",
        &[(91, "1"), (92, "2")],
        &[
            // Direct child — should be matched.
            ("Saga (2012)/Saga 001.cbz", None),
            // Nested under Saga (2012)/Annual/ — must NOT be picked up
            // by a directory: "Saga (2012)" request; the frontend would
            // group this under "Saga (2012)/Annual".
            ("Saga (2012)/Annual/Annual 1.cbz", None),
        ],
    )
    .await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/files/match-folder-from-cv",
            r#"{"directory": "Saga (2012)", "cv_volume_id": 9005}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["matched_count"], 1);
    assert_eq!(body["skipped"].as_array().unwrap().len(), 0);

    // The nested file is still unmatched.
    let nested = sqlx::query!(
        r#"SELECT status FROM files
           WHERE path_relative = 'Saga (2012)/Annual/Annual 1.cbz'"#
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(nested.status, "unmatched");
}

#[tokio::test]
async fn match_folder_strips_trailing_slash() {
    let app = build_test_app().await;
    setup_folder_match(
        &app,
        9004,
        "Saga",
        &[(91, "1")],
        &[("Saga (2012)/Saga 001.cbz", None)],
    )
    .await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/files/match-folder-from-cv",
            r#"{"directory": "Saga (2012)/", "cv_volume_id": 9004}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["matched_count"], 1);
}

// -------- /api/postprocess/pending --------

#[tokio::test]
async fn pending_endpoint_returns_empty_on_fresh_cache() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/api/postprocess/pending"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["count"], 0);
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn pending_endpoint_reflects_cache_contents() {
    use longbox_postprocess::{InterventionReason, PendingIntervention};
    let app = build_test_app().await;

    // Seed the shared cache directly — the test bypasses the consumer
    // task because it's verifying the HTTP shape, not the wiring (the
    // wiring is covered by lib-level tests in longbox-postprocess).
    app.state.pending_cache.push(PendingIntervention {
        source_path: std::path::PathBuf::from("/watch/Saga 001.cbz"),
        target_path: std::path::PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
        reason: InterventionReason::Conflict,
        size: 4096,
        last_attempt: time::OffsetDateTime::now_utc(),
    });
    app.state.pending_cache.push(PendingIntervention {
        source_path: std::path::PathBuf::from("/watch/Hellboy 002.cbz"),
        target_path: std::path::PathBuf::from("/lib/Hellboy (1994)/Hellboy (1994) 002.cbz"),
        reason: InterventionReason::MoveFailed("EXDEV cross-device".into()),
        size: 8192,
        last_attempt: time::OffsetDateTime::now_utc(),
    });

    let resp = app
        .request(empty_request("GET", "/api/postprocess/pending"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["count"], 2);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);

    let saga = items
        .iter()
        .find(|i| i["source_path"].as_str().unwrap().ends_with("Saga 001.cbz"))
        .expect("Saga item missing");
    assert_eq!(saga["reason"]["kind"], "conflict");
    assert_eq!(saga["size"], 4096);

    let hellboy = items
        .iter()
        .find(|i| {
            i["source_path"]
                .as_str()
                .unwrap()
                .ends_with("Hellboy 002.cbz")
        })
        .expect("Hellboy item missing");
    assert_eq!(hellboy["reason"]["kind"], "move_failed");
    assert_eq!(hellboy["reason"]["detail"], "EXDEV cross-device");
}

// -------- indexers (Phase A.8 Step 5) --------

#[tokio::test]
async fn indexers_create_lists_and_masks_the_api_key() {
    let app = build_test_app().await;

    let empty = response_json(app.request(empty_request("GET", "/api/indexers")).await).await;
    assert_eq!(empty.as_array().unwrap().len(), 0);

    let resp = app
        .request(json_request(
            "POST",
            "/api/indexers",
            r#"{"name":"NZBgeek","base_url":"https://api.nzbgeek.info","api_key":"SECRET","enabled":true,"priority":0,"maxage_days":1500}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["name"], "NZBgeek");
    assert_eq!(body["has_api_key"], true);
    // The key value is never serialized — only its presence.
    assert!(body.get("api_key").is_none());

    let list = response_json(app.request(empty_request("GET", "/api/indexers")).await).await;
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["has_api_key"], true);
    assert!(rows[0].get("api_key").is_none());
}

#[tokio::test]
async fn indexers_create_requires_an_api_key() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/indexers",
            r#"{"name":"NZBgeek","base_url":"https://x","api_key":"  "}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn indexers_create_duplicate_name_is_409() {
    let app = build_test_app().await;
    let make = || {
        json_request(
            "POST",
            "/api/indexers",
            r#"{"name":"NZBgeek","base_url":"https://x","api_key":"K"}"#,
        )
    };
    assert_eq!(app.request(make()).await.status(), StatusCode::OK);
    let resp = app.request(make()).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "conflict.indexer_exists");
}

#[tokio::test]
async fn indexers_update_with_blank_key_keeps_the_stored_key() {
    let app = build_test_app().await;
    let created = response_json(
        app.request(json_request(
            "POST",
            "/api/indexers",
            r#"{"name":"NZBgeek","base_url":"https://x","api_key":"ORIGINAL"}"#,
        ))
        .await,
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    // PUT with a blank api_key — the stored key must survive untouched.
    let resp = app
        .request(json_request(
            "PUT",
            &format!("/api/indexers/{id}"),
            r#"{"name":"NZBgeek Renamed","base_url":"https://x","api_key":"","enabled":false,"priority":2,"maxage_days":900}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let stored = longbox_db::indexer_config_repo::get(&app.state.db, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.api_key, "ORIGINAL");
    assert_eq!(stored.name, "NZBgeek Renamed");
    assert!(!stored.enabled);
}

#[tokio::test]
async fn indexers_update_unknown_id_is_404() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/indexers/9999",
            r#"{"name":"x","base_url":"https://x","api_key":"K"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn indexers_delete_then_delete_again_is_404() {
    let app = build_test_app().await;
    let created = response_json(
        app.request(json_request(
            "POST",
            "/api/indexers",
            r#"{"name":"NZBgeek","base_url":"https://x","api_key":"K"}"#,
        ))
        .await,
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    let del = app
        .request(empty_request("DELETE", &format!("/api/indexers/{id}")))
        .await;
    assert_eq!(del.status(), StatusCode::NO_CONTENT);

    let again = app
        .request(empty_request("DELETE", &format!("/api/indexers/{id}")))
        .await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn indexers_test_reports_ok_against_a_reachable_indexer() {
    let app = build_test_app().await;
    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("t", "search"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<rss><channel></channel></rss>"))
        .mount(&server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/indexers/test",
            format!(
                r#"{{"base_url":"{}","api_key":"K","name":"probe"}}"#,
                server.uri()
            ),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn indexers_test_reports_bad_credentials() {
    let app = build_test_app().await;
    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"<error code="100" description="Incorrect user credentials"/>"#),
        )
        .mount(&server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/indexers/test",
            format!(r#"{{"base_url":"{}","api_key":"BAD"}}"#, server.uri()),
        ))
        .await;
    // A failed connection is a *successful* test reporting ok:false.
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["ok"], false);
    assert!(body["message"].as_str().unwrap().contains("credentials"));
}

#[tokio::test]
async fn indexers_test_a_new_indexer_requires_a_key() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/indexers/test",
            r#"{"base_url":"https://x","api_key":""}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// -------- downloader (Phase A.8 Step 5) --------

#[tokio::test]
async fn downloader_get_is_null_until_configured_then_masks_the_secret() {
    let app = build_test_app().await;

    let unconfigured =
        response_json(app.request(empty_request("GET", "/api/downloader")).await).await;
    assert!(unconfigured.is_null());

    let resp = app
        .request(json_request(
            "PUT",
            "/api/downloader",
            r#"{"kind":"sab","base_url":"http://localhost:8080","secret":"APIKEY","category":"comics","enabled":true}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["kind"], "sab");
    assert_eq!(body["has_secret"], true);
    assert!(body.get("secret").is_none());

    let fetched = response_json(app.request(empty_request("GET", "/api/downloader")).await).await;
    assert_eq!(fetched["kind"], "sab");
    assert_eq!(fetched["has_secret"], true);
    assert!(fetched.get("secret").is_none());
}

#[tokio::test]
async fn downloader_nzbget_requires_a_username() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/downloader",
            r#"{"kind":"nzbget","base_url":"http://localhost:6789","secret":"pw"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn downloader_rejects_an_unknown_kind() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/downloader",
            r#"{"kind":"transmission","base_url":"http://x","secret":"s"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn downloader_update_with_blank_secret_keeps_the_stored_secret() {
    let app = build_test_app().await;
    app.request(json_request(
        "PUT",
        "/api/downloader",
        r#"{"kind":"sab","base_url":"http://localhost:8080","secret":"FIRSTKEY"}"#,
    ))
    .await;

    let resp = app
        .request(json_request(
            "PUT",
            "/api/downloader",
            r#"{"kind":"sab","base_url":"http://localhost:9090","secret":"","category":"x"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let stored = longbox_db::downloader_config_repo::get(&app.state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.secret, "FIRSTKEY");
    assert_eq!(stored.base_url, "http://localhost:9090");
}

#[tokio::test]
async fn downloader_delete_clears_the_config() {
    let app = build_test_app().await;
    app.request(json_request(
        "PUT",
        "/api/downloader",
        r#"{"kind":"sab","base_url":"http://x","secret":"K"}"#,
    ))
    .await;

    let del = app
        .request(empty_request("DELETE", "/api/downloader"))
        .await;
    assert_eq!(del.status(), StatusCode::NO_CONTENT);

    let after = response_json(app.request(empty_request("GET", "/api/downloader")).await).await;
    assert!(after.is_null());
}

#[tokio::test]
async fn downloader_test_reports_ok_against_a_reachable_sab() {
    let app = build_test_app().await;
    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue":{"slots":[]}}"#))
        .mount(&server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/downloader/test",
            format!(
                r#"{{"kind":"sab","base_url":"{}","secret":"K"}}"#,
                server.uri()
            ),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["ok"], true);
}

#[tokio::test]
async fn downloader_test_reports_a_bad_api_key() {
    let app = build_test_app().await;
    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string("error: API Key Incorrect"))
        .mount(&server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/downloader/test",
            format!(
                r#"{{"kind":"sab","base_url":"{}","secret":"BAD"}}"#,
                server.uri()
            ),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["ok"], false);
}

// -------- webhooks (Phase A.8 Step 5) --------

#[tokio::test]
async fn webhooks_create_list_and_update() {
    let app = build_test_app().await;

    let created = response_json(
        app.request(json_request(
            "POST",
            "/api/webhooks",
            r#"{"name":"Slack","url":"https://hooks.slack.com/services/x","event_mask":5,"enabled":true}"#,
        ))
        .await,
    )
    .await;
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["event_mask"], 5);

    let list = response_json(app.request(empty_request("GET", "/api/webhooks")).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    let updated = app
        .request(json_request(
            "PUT",
            &format!("/api/webhooks/{id}"),
            r#"{"name":"Slack prod","url":"https://hooks.slack.com/services/y","event_mask":15,"enabled":false}"#,
        ))
        .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let body = response_json(updated).await;
    assert_eq!(body["name"], "Slack prod");
    assert_eq!(body["event_mask"], 15);
    assert_eq!(body["enabled"], false);
}

#[tokio::test]
async fn webhooks_create_duplicate_name_is_409() {
    let app = build_test_app().await;
    let make = || {
        json_request(
            "POST",
            "/api/webhooks",
            r#"{"name":"Slack","url":"https://hooks.slack.com/x","event_mask":1}"#,
        )
    };
    assert_eq!(app.request(make()).await.status(), StatusCode::OK);
    let resp = app.request(make()).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(resp).await["error"]["code"],
        "conflict.webhook_exists"
    );
}

#[tokio::test]
async fn webhooks_reject_unknown_event_bits() {
    let app = build_test_app().await;
    // bit 16 is outside the known event mask (1|2|4|8 = 15).
    let resp = app
        .request(json_request(
            "POST",
            "/api/webhooks",
            r#"{"name":"Bad","url":"https://x","event_mask":16}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webhooks_reject_a_non_http_url() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/webhooks",
            r#"{"name":"Bad","url":"ftp://x","event_mask":1}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webhooks_update_unknown_id_is_404() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/webhooks/9999",
            r#"{"name":"x","url":"https://x","event_mask":1}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn webhooks_delete_removes_the_row() {
    let app = build_test_app().await;
    let created = response_json(
        app.request(json_request(
            "POST",
            "/api/webhooks",
            r#"{"name":"Slack","url":"https://hooks.slack.com/x","event_mask":1}"#,
        ))
        .await,
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    let del = app
        .request(empty_request("DELETE", &format!("/api/webhooks/{id}")))
        .await;
    assert_eq!(del.status(), StatusCode::NO_CONTENT);

    let list = response_json(app.request(empty_request("GET", "/api/webhooks")).await).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

// -------- pull engine (Phase A.8 Step 6) --------

#[tokio::test]
async fn pull_check_accepts_a_sweep_request() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("POST", "/api/pull/check")).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

// -------- pull list (Phase A.8 Step 7) --------

async fn seed_pull_series(app: &common::TestApp, title: &str) -> i64 {
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
            start_year: Some(2020),
            publisher: Some("Image".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn pull_list_add_lists_with_the_series_title() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;

    let empty = response_json(app.request(empty_request("GET", "/api/pull-list")).await).await;
    assert_eq!(empty.as_array().unwrap().len(), 0);

    let resp = app
        .request(json_request(
            "POST",
            "/api/pull-list",
            format!(r#"{{"series_id":{sid}}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let list = response_json(app.request(empty_request("GET", "/api/pull-list")).await).await;
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["series_id"], sid);
    assert_eq!(rows[0]["series_title"], "Saga");
    assert_eq!(rows[0]["paused"], false);
}

#[tokio::test]
async fn pull_list_add_unknown_series_is_404() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/pull-list",
            r#"{"series_id":9999}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pull_list_add_duplicate_is_409() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    let body = format!(r#"{{"series_id":{sid}}}"#);
    assert_eq!(
        app.request(json_request("POST", "/api/pull-list", body.clone()))
            .await
            .status(),
        StatusCode::OK
    );
    let resp = app
        .request(json_request("POST", "/api/pull-list", body))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(resp).await["error"]["code"],
        "conflict.already_on_pull_list"
    );
}

#[tokio::test]
async fn pull_list_get_one_reflects_subscription() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;

    let before = response_json(
        app.request(empty_request("GET", &format!("/api/pull-list/{sid}")))
            .await,
    )
    .await;
    assert!(before.is_null(), "not subscribed -> null");

    app.request(json_request(
        "POST",
        "/api/pull-list",
        format!(r#"{{"series_id":{sid}}}"#),
    ))
    .await;

    let after = response_json(
        app.request(empty_request("GET", &format!("/api/pull-list/{sid}")))
            .await,
    )
    .await;
    assert_eq!(after["series_id"], sid);
    assert_eq!(after["paused"], false);
}

#[tokio::test]
async fn pull_list_pause_then_resume() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    app.request(json_request(
        "POST",
        "/api/pull-list",
        format!(r#"{{"series_id":{sid}}}"#),
    ))
    .await;

    let paused = app
        .request(json_request(
            "PATCH",
            &format!("/api/pull-list/{sid}"),
            r#"{"paused":true}"#,
        ))
        .await;
    assert_eq!(paused.status(), StatusCode::OK);
    assert_eq!(response_json(paused).await["paused"], true);

    let resumed = app
        .request(json_request(
            "PATCH",
            &format!("/api/pull-list/{sid}"),
            r#"{"paused":false}"#,
        ))
        .await;
    assert_eq!(response_json(resumed).await["paused"], false);
}

#[tokio::test]
async fn pull_list_pause_unknown_series_is_404() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PATCH",
            "/api/pull-list/9999",
            r#"{"paused":true}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pull_list_delete_unsubscribes_then_404s() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    app.request(json_request(
        "POST",
        "/api/pull-list",
        format!(r#"{{"series_id":{sid}}}"#),
    ))
    .await;

    let del = app
        .request(empty_request("DELETE", &format!("/api/pull-list/{sid}")))
        .await;
    assert_eq!(del.status(), StatusCode::NO_CONTENT);

    let list = response_json(app.request(empty_request("GET", "/api/pull-list")).await).await;
    assert_eq!(list.as_array().unwrap().len(), 0);

    let again = app
        .request(empty_request("DELETE", &format!("/api/pull-list/{sid}")))
        .await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
}
