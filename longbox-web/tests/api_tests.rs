mod common;

use std::io::Write;
use std::path::Path;

use axum::http::StatusCode;
use common::{build_test_app, empty_request, json_request, response_json};
use longbox_db::{
    discovered_folders_repo, issue_repo, pull_attempt_repo, pull_list_repo, release_cache_repo,
    series_repo, webhook_config_repo, DiscoveredFolder, NewIssue, NewPullAttempt, NewPullEntry,
    NewReleaseCacheEntry, NewSeries, NewWebhookConfig,
};
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

// -------- reconcile (Library Tidy Step 4) --------
//
// One happy-path smoke test per route — every handler is exercised
// before commit. Step 7 layers on the failure / edge / idempotency
// matrix.

#[tokio::test]
async fn reconcile_phantoms_lists_zero_owned_series() {
    let app = build_test_app().await;
    // Seeded with no files -> zero-owned -> a phantom.
    let sid = seed_pull_series(&app, "Phantom Title").await;

    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/phantoms"))
            .await,
    )
    .await;
    let all = body["all_zero_owned"].as_array().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["id"], sid);
    // Never held files -> last_matched_count 0 -> not a transition phantom.
    assert_eq!(body["with_transition"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn reconcile_untracked_lists_discovered_folders() {
    let app = build_test_app().await;
    discovered_folders_repo::upsert(
        &app.state.db,
        DiscoveredFolder {
            folder_name: "Invincible (2003)".into(),
            file_count: 12,
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/untracked"))
            .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["folder_name"], "Invincible (2003)");
    assert_eq!(rows[0]["file_count"], 12);
}

#[tokio::test]
async fn reconcile_add_resolves_folder_and_dismisses_it() {
    let app = build_test_app().await;
    discovered_folders_repo::upsert(
        &app.state.db,
        DiscoveredFolder {
            folder_name: "The Walking Dead (2003)".into(),
            file_count: 5,
        },
    )
    .await
    .unwrap();

    Mock::given(method("GET"))
        .and(path("/volume/4050-2127/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
                "status_code": 1, "error": "OK", "number_of_total_results": 1,
                "results": {
                    "id": 2127, "name": "The Walking Dead", "start_year": "2003",
                    "publisher": { "id": 1, "name": "Image" },
                    "description": null,
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
                "number_of_total_results": 0, "limit": 100, "offset": 0,
                "results": []
            }"#,
        ))
        .mount(&app.cv_server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/add",
            r#"{"folders":[{"folder_name":"The Walking Dead (2003)","cv_id":2127}]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let succeeded = body["succeeded"].as_array().unwrap();
    assert_eq!(succeeded.len(), 1);
    assert_eq!(succeeded[0]["folder_name"], "The Walking Dead (2003)");
    assert_eq!(body["failed"].as_array().unwrap().len(), 0);

    // Series tracked, and the folder dismissed off the untracked list.
    assert!(series_repo::find_by_cv_id(&app.state.db, 2127)
        .await
        .unwrap()
        .is_some());
    let untracked = response_json(
        app.request(empty_request("GET", "/api/reconcile/untracked"))
            .await,
    )
    .await;
    assert_eq!(untracked.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn reconcile_dismiss_marks_folders_dismissed() {
    let app = build_test_app().await;
    discovered_folders_repo::upsert(
        &app.state.db,
        DiscoveredFolder {
            folder_name: "Saga (2012)".into(),
            file_count: 3,
        },
    )
    .await
    .unwrap();

    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/dismiss",
            r#"{"folder_names":["Saga (2012)"]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["dismissed"], 1);

    // Dismissed folders drop off the untracked list.
    let untracked = response_json(
        app.request(empty_request("GET", "/api/reconcile/untracked"))
            .await,
    )
    .await;
    assert_eq!(untracked.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn reconcile_delete_phantom_removes_zero_owned_series() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Doomed Series").await;

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/reconcile/phantom/{sid}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["deleted"], sid);

    assert!(series_repo::find_by_id(&app.state.db, sid)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn reconcile_bulk_delete_phantoms_removes_all_given() {
    let app = build_test_app().await;
    let a = seed_pull_series(&app, "Phantom A").await;
    let b = seed_pull_series(&app, "Phantom B").await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/phantoms/bulk",
            format!(r#"{{"series_ids":[{a},{b}]}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let mut deleted: Vec<i64> = body["deleted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    deleted.sort_unstable();
    let mut expected = [a, b];
    expected.sort_unstable();
    assert_eq!(deleted, expected);
    assert_eq!(body["skipped"].as_array().unwrap().len(), 0);

    assert!(series_repo::find_by_id(&app.state.db, a)
        .await
        .unwrap()
        .is_none());
    assert!(series_repo::find_by_id(&app.state.db, b)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn reconcile_keep_phantom_resets_last_matched_count() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Kept Series").await;
    // Bump into transition state (last_matched_count > 0).
    series_repo::update_last_matched_count(&app.state.db, sid, 7)
        .await
        .unwrap();

    let resp = app
        .request(empty_request(
            "POST",
            &format!("/api/reconcile/phantom/{sid}/keep"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["kept"], sid);

    // Reset to 0 -> still zero-owned, but no longer a transition phantom.
    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/phantoms"))
            .await,
    )
    .await;
    assert_eq!(body["with_transition"].as_array().unwrap().len(), 0);
    assert_eq!(body["all_zero_owned"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn reconcile_counts_reports_transition_and_untracked() {
    let app = build_test_app().await;
    // One transition phantom, one steady-state phantom, one untracked
    // folder. Counts should report 1 transition + 1 untracked.
    let transition = seed_pull_series(&app, "Transition").await;
    series_repo::update_last_matched_count(&app.state.db, transition, 3)
        .await
        .unwrap();
    seed_pull_series(&app, "Steady").await;
    discovered_folders_repo::upsert(
        &app.state.db,
        DiscoveredFolder {
            folder_name: "Invincible (2003)".into(),
            file_count: 4,
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/counts"))
            .await,
    )
    .await;
    assert_eq!(body["phantoms_with_transition"], 1);
    assert_eq!(body["untracked_folders"], 1);
}

// -------- reconcile: route matrix (Library Tidy Step 7) --------
//
// The failure / edge / idempotency / 404 coverage the Step 4-6 smoke
// tests deferred. Two e2e scenarios (Step 8, folded into Step 7) follow
// in the next section.

/// Mount ComicVine volume + (empty) issues mocks for `cv_id` — enough
/// for the reconcile-add flow's `fetch_volume` + `fetch_issues`.
async fn mount_cv_volume(app: &common::TestApp, cv_id: i64, name: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/volume/4050-{cv_id}/")))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"{{ "status_code": 1, "error": "OK", "number_of_total_results": 1,
                "results": {{ "id": {cv_id}, "name": "{name}", "start_year": "2010",
                "publisher": {{ "id": 1, "name": "Image" }}, "description": null,
                "image": {{ "medium_url": "https://example.com/c.jpg" }},
                "site_detail_url": "https://cv/{cv_id}/" }} }}"#
        )))
        .mount(&app.cv_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 1, "error": "OK", "number_of_total_results": 0,
                "limit": 100, "offset": 0, "results": [] }"#,
        ))
        .mount(&app.cv_server)
        .await;
}

/// Seed a series + one issue, drop a CBZ whose ComicInfo CV-URL resolves
/// to that issue, and run a full scan so the file lands `owned` and
/// present. Returns the series id. `cv_id` is the series' ComicVine
/// volume id (`None` when the test doesn't exercise cv_id linkage).
async fn seed_series_with_owned_file(
    app: &common::TestApp,
    title: &str,
    cv_id: Option<i64>,
    cv_issue_id: i64,
) -> i64 {
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id,
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
            start_year: Some(2010),
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
            cv_issue_id: Some(cv_issue_id),
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
    let comic_info = format!(
        "<?xml version=\"1.0\"?>\n<ComicInfo>\n  <Series>{title}</Series>\n  \
         <Number>1</Number>\n  \
         <Web>https://comicvine.gamespot.com/issue/4000-{cv_issue_id}/</Web>\n</ComicInfo>"
    );
    write_cbz(
        &app.library_path().join(format!("{title}/{title} 001.cbz")),
        Some(comic_info.as_str()),
    );
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    series.id
}

#[tokio::test]
async fn reconcile_convert_shallow_creates_series_and_attaches_files() {
    let app = build_test_app().await;
    // Two files for a series LongBox doesn't track — after a scan they
    // are unmatched and "Wytches (2014)" is a discovered folder.
    write_cbz(
        &app.library_path()
            .join("Wytches (2014)/Wytches (2014) 001.cbz"),
        None,
    );
    write_cbz(
        &app.library_path()
            .join("Wytches (2014)/Wytches (2014) 002.cbz"),
        None,
    );
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();

    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/convert",
            r#"{"folder_names":["Wytches (2014)"]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["results"][0]["status"], "converted");
    let series_id = body["results"][0]["series_id"].as_i64().unwrap();

    // Shallow series — title + year parsed from the folder, no cv_id.
    let series = series_repo::find_by_id(&app.state.db, series_id)
        .await
        .unwrap()
        .expect("series created");
    assert_eq!(series.title, "Wytches");
    assert_eq!(series.start_year, Some(2014));
    assert_eq!(series.cv_id, None);

    // Two number-only issues synthesized; both files attach as owned.
    let issues = issue_repo::list_by_series(&app.state.db, series_id)
        .await
        .unwrap();
    assert_eq!(issues.len(), 2);
    let owned = longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|f| f.status == "owned" && f.issue_id.is_some())
        .count();
    assert_eq!(owned, 2);

    // The folder is no longer untracked.
    let untracked = discovered_folders_repo::list(&app.state.db).await.unwrap();
    assert!(untracked.iter().all(|d| d.folder_name != "Wytches (2014)"));
}

#[tokio::test]
async fn reconcile_phantoms_partitions_transition_and_steady() {
    let app = build_test_app().await;
    let transition = seed_pull_series(&app, "Lost Series").await;
    series_repo::update_last_matched_count(&app.state.db, transition, 4)
        .await
        .unwrap();
    let steady = seed_pull_series(&app, "Never Owned").await;

    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/phantoms"))
            .await,
    )
    .await;
    let ids = |key: &str| -> Vec<i64> {
        body[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_i64().unwrap())
            .collect()
    };
    // Transition subset: only the bumped series. Full list: both.
    assert_eq!(ids("with_transition"), vec![transition]);
    let all = ids("all_zero_owned");
    assert_eq!(all.len(), 2);
    assert!(all.contains(&transition) && all.contains(&steady));
}

#[tokio::test]
async fn reconcile_add_partial_failure_splits_succeeded_and_failed() {
    let app = build_test_app().await;
    for name in ["Good (2010)", "Bad (2010)"] {
        discovered_folders_repo::upsert(
            &app.state.db,
            DiscoveredFolder {
                folder_name: name.into(),
                file_count: 1,
            },
        )
        .await
        .unwrap();
    }
    mount_cv_volume(&app, 3001, "Good").await;
    // The bad volume resolves to a ComicVine "object not found".
    Mock::given(method("GET"))
        .and(path("/volume/4050-3002/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 101, "error": "Object Not Found", "results": null }"#,
        ))
        .mount(&app.cv_server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/add",
            r#"{"folders":[{"folder_name":"Good (2010)","cv_id":3001},{"folder_name":"Bad (2010)","cv_id":3002}]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let succeeded = body["succeeded"].as_array().unwrap();
    let failed = body["failed"].as_array().unwrap();
    assert_eq!(succeeded.len(), 1);
    assert_eq!(succeeded[0]["folder_name"], "Good (2010)");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["folder_name"], "Bad (2010)");
}

#[tokio::test]
async fn reconcile_add_rejects_nonpositive_cv_id() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/add",
            r#"{"folders":[{"folder_name":"X (2010)","cv_id":0}]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["succeeded"].as_array().unwrap().len(), 0);
    let failed = body["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0]["error"].as_str().unwrap().contains("cv_id"));
}

#[tokio::test]
async fn reconcile_add_existing_cv_id_is_idempotent() {
    let app = build_test_app().await;
    // The series is already tracked under cv_id 4242.
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(4242),
            metron_id: None,
            title: "Tracked".into(),
            sort_title: "tracked".into(),
            start_year: Some(2010),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    discovered_folders_repo::upsert(
        &app.state.db,
        DiscoveredFolder {
            folder_name: "Tracked (2010)".into(),
            file_count: 3,
        },
    )
    .await
    .unwrap();

    // No CV mock — `add_or_get_from_cv` short-circuits on the existing
    // series without any ComicVine call.
    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/add",
            r#"{"folders":[{"folder_name":"Tracked (2010)","cv_id":4242}]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["succeeded"].as_array().unwrap().len(), 1);
    assert_eq!(body["failed"].as_array().unwrap().len(), 0);
    // The folder is dismissed even though no new series was created.
    let untracked = response_json(
        app.request(empty_request("GET", "/api/reconcile/untracked"))
            .await,
    )
    .await;
    assert_eq!(untracked.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn reconcile_add_unknown_folder_name_still_succeeds() {
    let app = build_test_app().await;
    mount_cv_volume(&app, 5001, "Fresh").await;
    // "Fresh (2010)" was never recorded in discovered_folders; the add
    // proceeds and the dismiss is a clean no-op (no 404).
    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/add",
            r#"{"folders":[{"folder_name":"Fresh (2010)","cv_id":5001}]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["succeeded"].as_array().unwrap().len(), 1);
    assert_eq!(body["failed"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn reconcile_dismiss_is_idempotent_and_ignores_unknown_names() {
    let app = build_test_app().await;
    for name in ["A (2010)", "B (2010)"] {
        discovered_folders_repo::upsert(
            &app.state.db,
            DiscoveredFolder {
                folder_name: name.into(),
                file_count: 1,
            },
        )
        .await
        .unwrap();
    }
    // First dismiss covers both known folders.
    let first = response_json(
        app.request(json_request(
            "POST",
            "/api/reconcile/dismiss",
            r#"{"folder_names":["A (2010)","B (2010)"]}"#,
        ))
        .await,
    )
    .await;
    assert_eq!(first["dismissed"], 2);
    // Re-dismissing the same names plus an unknown one is a clean no-op.
    let second = response_json(
        app.request(json_request(
            "POST",
            "/api/reconcile/dismiss",
            r#"{"folder_names":["A (2010)","B (2010)","Never Seen (2010)"]}"#,
        ))
        .await,
    )
    .await;
    assert_eq!(second["dismissed"], 0);
}

#[tokio::test]
async fn reconcile_delete_phantom_404_for_unknown_series() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("DELETE", "/api/reconcile/phantom/999999"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reconcile_delete_phantom_409_for_present_owned_files_not_absent() {
    let app = build_test_app().await;
    // Series A keeps a present, owned file — delete must 409.
    let present = seed_series_with_owned_file(&app, "Present", None, 600_001).await;
    // Series B owned a file that has since vanished — a transition
    // phantom. Delete must succeed: the guard counts `is_present = 1`
    // only, so an absent owned file never blocks the delete.
    let absent = seed_series_with_owned_file(&app, "Absent", None, 600_002).await;
    std::fs::remove_dir_all(app.library_path().join("Absent")).unwrap();
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();

    let blocked = app
        .request(empty_request(
            "DELETE",
            &format!("/api/reconcile/phantom/{present}"),
        ))
        .await;
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(blocked).await["error"]["code"],
        "conflict.series_has_owned_files"
    );

    let ok = app
        .request(empty_request(
            "DELETE",
            &format!("/api/reconcile/phantom/{absent}"),
        ))
        .await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert!(series_repo::find_by_id(&app.state.db, absent)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn reconcile_bulk_delete_skips_unknown_and_owned_with_readable_reasons() {
    let app = build_test_app().await;
    let phantom = seed_pull_series(&app, "Deletable Phantom").await;
    let owned = seed_series_with_owned_file(&app, "Has Files", None, 700_001).await;
    let unknown = 999_999_i64;

    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/phantoms/bulk",
            format!(r#"{{"series_ids":[{phantom},{owned},{unknown}]}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let deleted: Vec<i64> = body["deleted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(deleted, vec![phantom]);
    let skipped = body["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    // The skip reasons surface verbatim in UI toasts — assert they read
    // as human sentences, not merely that the field is populated.
    let reason_for = |sid: i64| -> String {
        skipped
            .iter()
            .find(|s| s["series_id"].as_i64() == Some(sid))
            .unwrap_or_else(|| panic!("no skipped entry for series {sid}"))["reason"]
            .as_str()
            .unwrap()
            .to_lowercase()
    };
    let owned_reason = reason_for(owned);
    assert!(
        owned_reason.contains("owned file"),
        "owned-files skip reason should mention owned files: {owned_reason:?}"
    );
    let unknown_reason = reason_for(unknown);
    assert!(
        unknown_reason.contains("not found"),
        "unknown-series skip reason should say not found: {unknown_reason:?}"
    );
}

#[tokio::test]
async fn reconcile_keep_404_for_unknown_series() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("POST", "/api/reconcile/phantom/999999/keep"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reconcile_counts_zero_when_nothing_to_reconcile() {
    let app = build_test_app().await;
    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/counts"))
            .await,
    )
    .await;
    assert_eq!(body["phantoms_with_transition"], 0);
    assert_eq!(body["untracked_folders"], 0);
}

// -------- reconcile: end-to-end (Library Tidy Step 8, folded in) --------
//
// The full scanner -> reconcile-API chain, exercising what the brief
// scoped as Step 8. Both run a real `scan_full`.

#[tokio::test]
async fn e2e_discover_folder_then_add_via_api() {
    let app = build_test_app().await;
    // The user drops an untracked series folder — unmatched CBZs, no
    // ComicInfo, resolving to no tracked series.
    write_cbz(
        &app.library_path()
            .join("Invincible (2003)/Invincible 001.cbz"),
        None,
    );
    write_cbz(
        &app.library_path()
            .join("Invincible (2003)/Invincible 002.cbz"),
        None,
    );

    // A full scan walks the library and records the folder as discovered.
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let untracked = response_json(
        app.request(empty_request("GET", "/api/reconcile/untracked"))
            .await,
    )
    .await;
    let rows = untracked.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["folder_name"], "Invincible (2003)");

    // The user resolves it to a ComicVine volume via the reconcile API.
    mount_cv_volume(&app, 8001, "Invincible").await;
    let add = app
        .request(json_request(
            "POST",
            "/api/reconcile/add",
            r#"{"folders":[{"folder_name":"Invincible (2003)","cv_id":8001}]}"#,
        ))
        .await;
    assert_eq!(add.status(), StatusCode::OK);
    assert_eq!(
        response_json(add).await["succeeded"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // The catalog now tracks the series and the folder is off the
    // untracked list.
    assert!(series_repo::find_by_cv_id(&app.state.db, 8001)
        .await
        .unwrap()
        .is_some());
    let after = response_json(
        app.request(empty_request("GET", "/api/reconcile/untracked"))
            .await,
    )
    .await;
    assert_eq!(after.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn e2e_delete_folder_transitions_to_phantom_then_remove() {
    let app = build_test_app().await;
    // A tracked series with a matched, owned, present file on disk.
    let series = seed_series_with_owned_file(&app, "Chew", None, 900_001).await;
    // Not a phantom yet — its file is present.
    let phantoms = response_json(
        app.request(empty_request("GET", "/api/reconcile/phantoms"))
            .await,
    )
    .await;
    assert_eq!(phantoms["all_zero_owned"].as_array().unwrap().len(), 0);

    // The user deletes the folder; the next scan walks it, marks the
    // file missing, and the series transitions to a phantom.
    std::fs::remove_dir_all(app.library_path().join("Chew")).unwrap();
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();

    let phantoms = response_json(
        app.request(empty_request("GET", "/api/reconcile/phantoms"))
            .await,
    )
    .await;
    let with_transition: Vec<i64> = phantoms["with_transition"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    assert_eq!(
        with_transition,
        vec![series],
        "Chew is a transition phantom after losing its files"
    );

    // The user removes it from the catalog via the reconcile API.
    let del = app
        .request(empty_request(
            "DELETE",
            &format!("/api/reconcile/phantom/{series}"),
        ))
        .await;
    assert_eq!(del.status(), StatusCode::OK);
    assert!(series_repo::find_by_id(&app.state.db, series)
        .await
        .unwrap()
        .is_none());
}

// -------- release calendar (A.8 Step 8) --------

/// Mount a ComicVine release-calendar (`store_date`-filtered `/issues/`)
/// response with the given results array for the `from`–`to` range.
async fn mount_cv_calendar(app: &common::TestApp, from: &str, to: &str, results: &str) {
    // One `issue_number` per issue — the nested `volume` ref has none,
    // so this counts issues without over-counting `"id"`.
    let count = results.matches("\"issue_number\"").count();
    let body = format!(
        r#"{{ "status_code": 1, "error": "OK", "limit": 100, "offset": 0,
            "number_of_page_results": {count}, "number_of_total_results": {count},
            "results": {results} }}"#
    );
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .and(query_param("filter", format!("store_date:{from}|{to}")))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&app.cv_server)
        .await;
}

#[tokio::test]
async fn calendar_queries_cv_cold_and_enriches_pull_list_state() {
    let app = build_test_app().await;
    // One calendar volume is a tracked, subscribed series; the other is
    // untracked.
    let tracked = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(4050),
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
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: tracked.id,
            start_issue: None,
        },
    )
    .await
    .unwrap();

    mount_cv_calendar(
        &app,
        "2026-05-13",
        "2026-05-19",
        r#"[
            { "id": 50001, "issue_number": "12", "name": null, "cover_date": "2026-07-01",
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 4050, "name": "Saga" }, "site_detail_url": "https://cv/4000-50001/" },
            { "id": 50002, "issue_number": "3", "name": null, "cover_date": null,
              "store_date": "2026-05-15", "description": null, "image": null,
              "volume": { "id": 9999, "name": "Untracked Title" },
              "site_detail_url": "https://cv/4000-50002/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request(
            "GET",
            "/api/releases/calendar?from=2026-05-13&to=2026-05-19",
        ))
        .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let saga = rows.iter().find(|r| r["cv_volume_id"] == 4050).unwrap();
    assert_eq!(saga["series_id"], tracked.id);
    assert_eq!(saga["on_pull_list"], true);
    assert_eq!(saga["volume_name"], "Saga");
    let untracked = rows.iter().find(|r| r["cv_volume_id"] == 9999).unwrap();
    assert!(untracked["series_id"].is_null());
    assert_eq!(untracked["on_pull_list"], false);
}

#[tokio::test]
async fn calendar_serves_a_fresh_cache_entry_without_hitting_cv() {
    let app = build_test_app().await;
    // Prime the cache. With no CV mock mounted, a cache miss would fall
    // through to CV, 404, and surface as 502 — so a 200 proves the hit.
    release_cache_repo::upsert(
        &app.state.db,
        NewReleaseCacheEntry {
            date_from: "2026-05-13".into(),
            date_to: "2026-05-19".into(),
            publisher: String::new(),
            payload_json: r#"[{"cv_issue_id":111,"issue_number":"1","store_date":"2026-05-14",
                "cv_volume_id":4050,"volume_name":"Saga","cover_url":null,
                "site_detail_url":"https://cv/4000-111/"}]"#
                .into(),
        },
    )
    .await
    .unwrap();

    let resp = app
        .request(empty_request(
            "GET",
            "/api/releases/calendar?from=2026-05-13&to=2026-05-19",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["cv_issue_id"], 111);
}

#[tokio::test]
async fn calendar_refresh_requeries_cv_past_a_fresh_cache() {
    let app = build_test_app().await;
    // A fresh cache entry holds issue 111 ...
    release_cache_repo::upsert(
        &app.state.db,
        NewReleaseCacheEntry {
            date_from: "2026-05-13".into(),
            date_to: "2026-05-19".into(),
            publisher: String::new(),
            payload_json: r#"[{"cv_issue_id":111,"issue_number":"1","store_date":"2026-05-14",
                "cv_volume_id":4050,"volume_name":"Saga","cover_url":null,
                "site_detail_url":"https://cv/4000-111/"}]"#
                .into(),
        },
    )
    .await
    .unwrap();
    // ... but CV now reports issue 222.
    mount_cv_calendar(
        &app,
        "2026-05-13",
        "2026-05-19",
        r#"[
            { "id": 222, "issue_number": "2", "name": null, "cover_date": null,
              "store_date": "2026-05-16", "description": null, "image": null,
              "volume": { "id": 4050, "name": "Saga" }, "site_detail_url": "https://cv/4000-222/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request(
            "GET",
            "/api/releases/calendar?from=2026-05-13&to=2026-05-19&refresh=true",
        ))
        .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["cv_issue_id"], 222,
        "refresh bypassed the fresh cache"
    );
}

#[tokio::test]
async fn calendar_rejects_a_malformed_date() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request(
            "GET",
            "/api/releases/calendar?from=not-a-date&to=2026-05-19",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn calendar_add_to_pull_list_creates_series_and_subscribes() {
    let app = build_test_app().await;
    mount_cv_volume(&app, 2127, "The Walking Dead").await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"cv_volume_id":2127}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let series_id = response_json(resp).await["series_id"].as_i64().unwrap();

    // The volume is now a tracked series and that series is subscribed.
    let series = series_repo::find_by_cv_id(&app.state.db, 2127)
        .await
        .unwrap()
        .expect("series created");
    assert_eq!(series.id, series_id);
    assert!(pull_list_repo::get(&app.state.db, series_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn calendar_add_to_pull_list_is_idempotent_for_an_already_subscribed_series() {
    let app = build_test_app().await;
    // The volume is already a tracked, subscribed series.
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(5151),
            metron_id: None,
            title: "Chew".into(),
            sort_title: "chew".into(),
            start_year: Some(2009),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: series.id,
            start_issue: None,
        },
    )
    .await
    .unwrap();

    // No CV mock — add_or_get_from_cv short-circuits on the existing series.
    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"cv_volume_id":5151}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["series_id"], series.id);
    // Still exactly one pull-list entry — no duplicate, no 409.
    assert_eq!(
        pull_list_repo::list_all(&app.state.db).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn calendar_bulk_add_returns_per_volume_status() {
    let app = build_test_app().await;
    // Two volumes already tracked (so add_or_get_from_cv short-circuits,
    // no CV mock needed): one unsubscribed, one already on the pull list.
    let unsub = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(6001),
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
    let subbed = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(6002),
            metron_id: None,
            title: "Chew".into(),
            sort_title: "chew".into(),
            start_year: Some(2009),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: subbed.id,
            start_issue: None,
        },
    )
    .await
    .unwrap();

    // 6001 -> added, 6002 -> already_on_list, 0 -> failed (must be > 0).
    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull/bulk",
            r#"{"cv_volume_ids":[6001,6002,0]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    let by_id = |cid: i64| {
        results
            .iter()
            .find(|r| r["cv_volume_id"].as_i64() == Some(cid))
            .unwrap_or_else(|| panic!("no result for cv_volume_id {cid}"))
    };
    assert_eq!(by_id(6001)["status"], "added");
    assert_eq!(by_id(6001)["series_id"], unsub.id);
    assert_eq!(by_id(6002)["status"], "already_on_list");
    assert_eq!(by_id(6002)["series_id"], subbed.id);
    assert_eq!(by_id(0)["status"], "failed");
    assert!(by_id(0)["error"].is_string());

    // 6001 is subscribed now; 6002 still has exactly its one entry.
    assert!(pull_list_repo::get(&app.state.db, unsub.id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        pull_list_repo::list_all(&app.state.db).await.unwrap().len(),
        2
    );
}

// -------- releases of note (A.8 Step 9) --------

/// Mount a ComicVine `/issues/` response for ANY query. The of-note
/// endpoint computes its ship-week range server-side, so a test can't
/// match on a fixed `filter` param the way `mount_cv_calendar` does.
async fn mount_cv_calendar_any(app: &common::TestApp, results: &str) {
    let count = results.matches("\"issue_number\"").count();
    let body = format!(
        r#"{{ "status_code": 1, "error": "OK", "limit": 100, "offset": 0,
            "number_of_page_results": {count}, "number_of_total_results": {count},
            "results": {results} }}"#
    );
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&app.cv_server)
        .await;
}

#[tokio::test]
async fn releases_of_note_surfaces_owned_unpulled_matches() {
    let app = build_test_app().await;
    // The user owns "Saga" — give it a matched, owned file.
    seed_series_with_owned_file(&app, "Saga", None, 810_001).await;
    // This week's calendar: a Saga issue (name-matches) and an unrelated one.
    mount_cv_calendar_any(
        &app,
        r#"[
            { "id": 81001, "issue_number": "55", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 8100, "name": "Saga" },
              "site_detail_url": "https://cv/4000-81001/" },
            { "id": 81002, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 8200, "name": "Random Comic" },
              "site_detail_url": "https://cv/4000-81002/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/releases/of-note"))
            .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the owned-series match surfaces");
    assert_eq!(rows[0]["cv_volume_id"], 8100);
    assert_eq!(rows[0]["volume_name"], "Saga");
    assert_eq!(rows[0]["issue_count"], 1);
}

#[tokio::test]
async fn releases_of_note_excludes_a_volume_on_the_pull_list() {
    let app = build_test_app().await;
    // The user owns "Saga" (cv_id 8800) and it is already on the pull list.
    let series = seed_series_with_owned_file(&app, "Saga", Some(8800), 820_001).await;
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: series,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    mount_cv_calendar_any(
        &app,
        r#"[
            { "id": 82001, "issue_number": "56", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 8800, "name": "Saga" },
              "site_detail_url": "https://cv/4000-82001/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/releases/of-note"))
            .await,
    )
    .await;
    // Name-matches an owned series, but it is already pulled — not "of note".
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn releases_of_note_dedups_a_volume_with_several_issues() {
    let app = build_test_app().await;
    seed_series_with_owned_file(&app, "Saga", None, 830_001).await;
    // Two Saga issues land in the week — one row, issue_count 2.
    mount_cv_calendar_any(
        &app,
        r#"[
            { "id": 83001, "issue_number": "57", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 9100, "name": "Saga" },
              "site_detail_url": "https://cv/4000-83001/" },
            { "id": 83002, "issue_number": "58", "name": null, "cover_date": null,
              "store_date": "2026-05-15", "description": null, "image": null,
              "volume": { "id": 9100, "name": "Saga" },
              "site_detail_url": "https://cv/4000-83002/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/releases/of-note"))
            .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cv_volume_id"], 9100);
    assert_eq!(rows[0]["issue_count"], 2);
}

#[tokio::test]
async fn releases_of_note_empty_when_nothing_owned_matches() {
    let app = build_test_app().await;
    // The user owns "Chew" — nothing in this week's calendar matches it.
    seed_series_with_owned_file(&app, "Chew", None, 840_001).await;
    mount_cv_calendar_any(
        &app,
        r#"[
            { "id": 84001, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 8400, "name": "Saga" },
              "site_detail_url": "https://cv/4000-84001/" },
            { "id": 84002, "issue_number": "2", "name": null, "cover_date": null,
              "store_date": "2026-05-15", "description": null, "image": null,
              "volume": { "id": 8500, "name": "Batman" },
              "site_detail_url": "https://cv/4000-84002/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/releases/of-note"))
            .await,
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// -------- this week's pulls (A.8 Step 9b) --------

/// Insert a tracked series with the given `cv_id`. Returns its series id.
async fn seed_tracked_series(app: &common::TestApp, title: &str, cv_id: i64) -> i64 {
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(cv_id),
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
            start_year: Some(2012),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn this_weeks_pulls_lists_only_pulled_volumes() {
    let app = build_test_app().await;
    // Volume 7700 is a subscribed series; 8800 is tracked but not pulled.
    let pulled = seed_tracked_series(&app, "Saga", 7700).await;
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: pulled,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    seed_tracked_series(&app, "Chew", 8800).await;

    // This week's calendar: two Saga issues + one Chew issue.
    mount_cv_calendar_any(
        &app,
        r#"[
            { "id": 91001, "issue_number": "60", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 7700, "name": "Saga" },
              "site_detail_url": "https://cv/4000-91001/" },
            { "id": 91002, "issue_number": "61", "name": null, "cover_date": null,
              "store_date": "2026-05-15", "description": null, "image": null,
              "volume": { "id": 7700, "name": "Saga" },
              "site_detail_url": "https://cv/4000-91002/" },
            { "id": 91003, "issue_number": "9", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 8800, "name": "Chew" },
              "site_detail_url": "https://cv/4000-91003/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/releases/this-weeks-pulls"))
            .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    // Both Saga issues (per-issue, no dedup); the unpulled Chew issue is out.
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r["cv_volume_id"] == 7700));
}

#[tokio::test]
async fn this_weeks_pulls_empty_when_nothing_is_subscribed() {
    let app = build_test_app().await;
    // A tracked series, but not on the pull list.
    seed_tracked_series(&app, "Chew", 8800).await;
    mount_cv_calendar_any(
        &app,
        r#"[
            { "id": 92001, "issue_number": "9", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 8800, "name": "Chew" },
              "site_detail_url": "https://cv/4000-92001/" }
        ]"#,
    )
    .await;

    let body = response_json(
        app.request(empty_request("GET", "/api/releases/this-weeks-pulls"))
            .await,
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

// -------- webhook test endpoint (A.8 Step 10) --------

#[tokio::test]
async fn webhook_test_delivers_to_the_configured_url() {
    let app = build_test_app().await;
    // A fresh mock server stands in for the user's webhook endpoint.
    let target = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&target)
        .await;
    let webhook = webhook_config_repo::insert(
        &app.state.db,
        NewWebhookConfig {
            name: "Test Hook".into(),
            url: format!("{}/hook", target.uri()),
            event_mask: 1,
            enabled: true,
        },
    )
    .await
    .unwrap();

    let resp = app
        .request(empty_request(
            "POST",
            &format!("/api/webhooks/{}/test", webhook.id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["delivered"], true);
    assert_eq!(target.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn webhook_test_404_for_unknown_id() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("POST", "/api/webhooks/999999/test"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// -------- needs-attention pull failures (A.8 Step 11) --------

/// Insert a series + one issue; returns (series_id, issue_id).
async fn seed_series_and_issue(app: &common::TestApp, title: &str, number: &str) -> (i64, i64) {
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
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
            number: number.into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    (series.id, issue.id)
}

fn failed_attempt(series_id: i64, issue_id: i64, release_id: Option<&str>) -> NewPullAttempt {
    NewPullAttempt {
        series_id,
        issue_id,
        indexer_id: None,
        release_id: release_id.map(str::to_owned),
        status: "failed".into(),
        error_message: Some("boom".into()),
        retry_count: 3,
        download_handle: None,
    }
}

#[tokio::test]
async fn needs_attention_pull_failures_lists_categorized_failures() {
    let app = build_test_app().await;
    let (series, issue_a) = seed_series_and_issue(&app, "Saga", "1").await;
    let issue_b = issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: series,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "2".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    // A release_id-less failure is a submission failure; one with a
    // release is a grab failure.
    pull_attempt_repo::insert(&app.state.db, failed_attempt(series, issue_a, None))
        .await
        .unwrap();
    pull_attempt_repo::insert(
        &app.state.db,
        failed_attempt(series, issue_b, Some("rel-1")),
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", "/api/needs-attention/pull-failures"))
            .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let category_of = |iid: i64| -> String {
        rows.iter()
            .find(|r| r["issue_id"].as_i64() == Some(iid))
            .unwrap()["category"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(category_of(issue_a), "submission_failed");
    assert_eq!(category_of(issue_b), "grab_failed");
}

#[tokio::test]
async fn needs_attention_pull_failures_excludes_a_retried_issue() {
    let app = build_test_app().await;
    let (series, issue) = seed_series_and_issue(&app, "Chew", "1").await;
    // A failure, then a later attempt that is back in flight.
    pull_attempt_repo::insert(&app.state.db, failed_attempt(series, issue, None))
        .await
        .unwrap();
    pull_attempt_repo::insert(
        &app.state.db,
        NewPullAttempt {
            series_id: series,
            issue_id: issue,
            indexer_id: None,
            release_id: Some("rel-2".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 1,
            download_handle: Some("handle".into()),
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", "/api/needs-attention/pull-failures"))
            .await,
    )
    .await;
    // The issue's latest attempt is `submitted` — not "needs attention".
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn needs_attention_retry_clears_an_issues_failed_attempts() {
    let app = build_test_app().await;
    let (series, issue) = seed_series_and_issue(&app, "Saga", "1").await;
    pull_attempt_repo::insert(&app.state.db, failed_attempt(series, issue, None))
        .await
        .unwrap();

    let resp = app
        .request(json_request(
            "POST",
            "/api/needs-attention/retry",
            format!(r#"{{"series_id":{series},"issue_id":{issue}}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["cleared"], 1);

    // Un-parked — gone from the failures list.
    let body = response_json(
        app.request(empty_request("GET", "/api/needs-attention/pull-failures"))
            .await,
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}
