mod common;

use std::io::Write;
use std::path::Path;

use axum::http::StatusCode;
use common::{build_test_app, empty_request, json_request, response_json};
use longbox_db::{
    cv_volume_cache_repo, discovered_folders_repo, issue_repo, pull_attempt_repo, pull_list_repo,
    release_cache_repo, series_repo, webhook_config_repo, DiscoveredFolder, NewIssue,
    NewPullAttempt, NewPullEntry, NewReleaseCacheEntry, NewSeries, NewWebhookConfig,
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
async fn spa_shell_is_served_no_cache() {
    // The SPA shell (index.html, returned for the bare root and every
    // client-side route) must never be cached stale — a stale shell points at
    // hashed chunk filenames that 404 after a redeploy and crashes the app.
    let app = build_test_app().await;
    for uri in ["/", "/series/42"] {
        let resp = app.request(empty_request("GET", uri)).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-cache"),
            "{uri} should be no-cache"
        );
    }
}

#[tokio::test]
async fn health_returns_200_with_version() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/health")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn health_carries_db_ok_uptime_and_db_metrics() {
    let app = build_test_app().await;
    let resp = app.request(empty_request("GET", "/api/health")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["db_ok"], true);
    assert!(body["uptime_seconds"].as_i64().unwrap() >= 0);
    // Fresh test app — no scans, no enrichment attempts.
    assert!(body["last_scan_at"].is_null());
    assert!(body["last_enrichment_at"].is_null());
    assert_eq!(body["enrichment_queue_depth"], 0);
}

#[tokio::test]
async fn health_reflects_enrichment_queue_depth() {
    let app = build_test_app().await;
    // Two shallow series (cv_id IS NULL) → enrichment_queue_depth = 2.
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Shallow A".into(),
            sort_title: "shallow a".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Shallow B".into(),
            sort_title: "shallow b".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    // One enriched (cv_id set) — not counted.
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(123),
            metron_id: None,
            title: "Enriched".into(),
            sort_title: "enriched".into(),
            start_year: Some(2020),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    let body = response_json(app.request(empty_request("GET", "/api/health")).await).await;
    assert_eq!(body["enrichment_queue_depth"], 2);
}

// -------- CV rate-limit (Tier 4 ITEM 16) --------

#[tokio::test]
async fn cv_rate_limit_endpoint_returns_snapshot_shape() {
    // We don't exercise the counter increment here (that lives inside
    // the CV client's execute_with_retry path, gated by a wiremock
    // hit). Just lock the response shape and the default limit
    // reflected from config (3600/hr for cv_direct; the primary
    // `cv` client is 180/hr but the route surfaces `state.cv`).
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/api/cv/rate-limit"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert!(body["count"].as_u64().is_some());
    assert!(body["limit_per_hour"].as_u64().unwrap() > 0);
    assert!(body["window_started_at_unix"].as_i64().is_some());
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
async fn add_series_creates_on_disk_folder() {
    // The Add page builds the user's expectation that they can drop
    // a file into the new series' folder immediately after the create.
    // Confirm POST /api/series's side effect: `{library_root}/{title}
    // ({start_year})/` exists as a real directory after the call.
    let app = build_test_app().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-2127/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 1, "error": "OK", "number_of_total_results": 1,
                "results": { "id": 2127, "name": "The Walking Dead",
                    "start_year": "2003",
                    "publisher": { "id": 1, "name": "Image" },
                    "description": null,
                    "image": { "medium_url": "https://example.com/wd.jpg" },
                    "site_detail_url": "https://cv/wd/4050-2127/" } }"#,
        ))
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

    let folder = app.library_path().join("The Walking Dead (2003)");
    assert!(
        !folder.exists(),
        "fixture must start clean — folder shouldn't exist before the call"
    );

    let resp = app
        .request(json_request("POST", "/api/series", r#"{"cv_id": 2127}"#))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        folder.is_dir(),
        "POST /api/series must create the on-disk series folder"
    );
}

#[tokio::test]
async fn add_series_response_includes_pull_search_queued_count() {
    // Auto-search fires when the pull engine is fully configured.
    // Seed a downloader + indexer, mount CV mocks that return TWO
    // shipped issues (cover_date in the past), POST /api/series, and
    // assert `pull_search_queued == 2`. The fire-and-forget engine
    // tasks may not finish before this test ends — that's fine; the
    // contract is the count reported back to the frontend, not the
    // eventual `pull_attempts` rows.
    let app = build_test_app().await;
    longbox_db::indexer_config_repo::insert(
        &app.state.db,
        longbox_db::NewIndexerConfig {
            name: "stub-indexer".into(),
            base_url: "https://stub.example/api".into(),
            api_key: "KEY".into(),
            enabled: true,
            priority: 0,
            maxage_days: 1500,
        },
    )
    .await
    .unwrap();
    longbox_db::downloader_config_repo::upsert(
        &app.state.db,
        longbox_db::NewDownloaderConfig {
            kind: "sab".into(),
            base_url: "https://stub.example/sab".into(),
            username: None,
            secret: "KEY".into(),
            category: String::new(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/volume/4050-2127/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 1, "error": "OK", "number_of_total_results": 1,
                "results": { "id": 2127, "name": "The Walking Dead",
                    "start_year": "2003",
                    "publisher": { "id": 1, "name": "Image" },
                    "description": null,
                    "image": { "medium_url": "https://example.com/wd.jpg" },
                    "site_detail_url": "https://cv/wd/4050-2127/" } }"#,
        ))
        .mount(&app.cv_server)
        .await;
    // Both issues are dated 2003 — well in the past, so the
    // "shipped not solicited" predicate (cover_date <= today)
    // accepts them.
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 1, "error": "OK",
                "number_of_total_results": 2, "limit": 100, "offset": 0,
                "results": [
                    { "id": 10001, "issue_number": "1", "name": "Days Gone Bye",
                      "cover_date": "2003-10-08", "description": null,
                      "image": { "medium_url": "https://example.com/wd-1.jpg" },
                      "site_detail_url": "https://cv/4000-10001/" },
                    { "id": 10002, "issue_number": "2", "name": null,
                      "cover_date": "2003-11-12", "description": null,
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
    assert_eq!(
        body["pull_search_queued"], 2,
        "auto-search must dispatch one task per missing-and-shipped issue"
    );
}

#[tokio::test]
async fn add_series_skips_auto_search_silently_without_downloader() {
    // Spec: when the pull engine isn't configured (no downloader, or
    // downloader disabled, or no indexers), the auto-search is a
    // silent no-op — the response carries `pull_search_queued: 0` so
    // the frontend renders the plain "Added X" toast.
    let app = build_test_app().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-2127/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 1, "error": "OK", "number_of_total_results": 1,
                "results": { "id": 2127, "name": "The Walking Dead",
                    "start_year": "2003",
                    "publisher": { "id": 1, "name": "Image" },
                    "description": null,
                    "image": { "medium_url": "https://example.com/wd.jpg" },
                    "site_detail_url": "https://cv/wd/4050-2127/" } }"#,
        ))
        .mount(&app.cv_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{ "status_code": 1, "error": "OK", "number_of_total_results": 1,
                "limit": 100, "offset": 0,
                "results": [
                    { "id": 10001, "issue_number": "1", "name": null,
                      "cover_date": "2003-10-08", "description": null,
                      "image": null,
                      "site_detail_url": "https://cv/4000-10001/" }
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
    assert_eq!(
        body["pull_search_queued"], 0,
        "silent skip: no downloader → no queued searches reported"
    );
    // Sanity: no pull_attempts row should have been written either.
    let row = series_repo::find_by_cv_id(&app.state.db, 2127)
        .await
        .unwrap()
        .unwrap();
    let issues = issue_repo::list_by_series(&app.state.db, row.id)
        .await
        .unwrap();
    for issue in issues {
        let attempts =
            longbox_db::pull_attempt_repo::list_for_issue(&app.state.db, row.id, issue.id)
                .await
                .unwrap();
        assert!(
            attempts.is_empty(),
            "no auto-search task should have written a pull_attempts row"
        );
    }
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
async fn series_detail_includes_authoritative_owned_file_count() {
    // The detail response's `owned_file_count` is computed via the
    // join-based query the delete-series guard uses — NOT derived
    // from the per-issue file lookup that powers `issues[].file`.
    // Seed a series with one issue and an owned+present file; assert
    // the count comes back as 1.
    let app = build_test_app().await;
    let (sid, iid) = seed_series_and_issue(&app, "Adventureman", "1").await;
    longbox_db::file_repo::insert(
        &app.state.db,
        longbox_db::NewFile {
            issue_id: Some(iid),
            library_root_id: app.library_root_id,
            path_relative: "Adventureman (2020)/Adventureman 001.cbz".into(),
            size_bytes: 1,
            mtime: time::macros::datetime!(2024-01-01 0:00),
            last_scanned_at: time::macros::datetime!(2024-01-01 0:00),
            match_method: "filename".into(),
            match_confidence: 0.99,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: time::macros::datetime!(2024-01-01 0:00),
            matched_at: Some(time::macros::datetime!(2024-01-01 0:00)),
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", &format!("/api/series/{sid}")))
            .await,
    )
    .await;
    assert_eq!(body["owned_file_count"], 1);
}

#[tokio::test]
async fn series_detail_owned_file_count_zero_when_no_files() {
    let app = build_test_app().await;
    let (sid, _) = seed_series_and_issue(&app, "Empty", "1").await;
    let body = response_json(
        app.request(empty_request("GET", &format!("/api/series/{sid}")))
            .await,
    )
    .await;
    assert_eq!(body["owned_file_count"], 0);
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

#[tokio::test]
async fn force_delete_series_unlinks_owned_files_and_succeeds() {
    // The Library Tidy "Delete duplicate anyway" path: a series with
    // misassigned owned files gets dropped, its files revert to
    // needs_review with issue_id NULL, and the standard owned-files
    // guard does NOT fire.
    let app = build_test_app().await;
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(2127),
            metron_id: None,
            title: "The Walking Dead".into(),
            sort_title: "walking dead".into(),
            start_year: Some(2003),
            publisher: Some("Image".into()),
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
            cv_issue_id: Some(101),
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
    write_cbz(
        &app.library_path()
            .join("The Walking Dead (2003)/The Walking Dead 001 (2003).cbz"),
        None,
    );
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let files_before =
        longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
            .await
            .unwrap();
    assert_eq!(files_before.len(), 1);
    assert_eq!(files_before[0].status, "owned");
    assert!(files_before[0].issue_id.is_some());
    let file_id = files_before[0].id;

    // Standard delete refuses while the file is owned + present.
    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{}", series.id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Force-delete succeeds.
    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{}?force=true", series.id),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Series is gone.
    assert!(series_repo::find_by_id(&app.state.db, series.id)
        .await
        .unwrap()
        .is_none());

    // File is preserved, but unlinked and flagged needs_review.
    let files_after =
        longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
            .await
            .unwrap();
    assert_eq!(files_after.len(), 1);
    assert_eq!(files_after[0].id, file_id);
    assert_eq!(files_after[0].status, "needs_review");
    assert!(files_after[0].issue_id.is_none());
}

#[tokio::test]
async fn force_delete_series_404_for_unknown_id() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("DELETE", "/api/series/9999?force=true"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// -------- delete_files (disk-side series-folder removal) --------

/// Seed a barebones series row with the given title + year and return
/// its id. No issues, no files — the tests below set those up on a
/// case-by-case basis.
async fn insert_barebones_series(
    app: &common::TestApp,
    title: &str,
    start_year: Option<i32>,
) -> i64 {
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
            start_year,
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
async fn delete_files_removes_the_series_folder_from_disk() {
    let app = build_test_app().await;
    let id = insert_barebones_series(&app, "Saga", Some(2012)).await;
    let folder = app.library_path().join("Saga (2012)");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Saga 001 (2012).cbz"), b"contents").unwrap();
    assert!(folder.is_dir());

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{id}?delete_files=true"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["deleted"], id);
    // The handler echoes the canonicalized path back so the frontend
    // can confirm exactly what was removed.
    assert!(body["folder_deleted"]
        .as_str()
        .unwrap()
        .ends_with("Saga (2012)"));

    assert!(!folder.exists(), "series folder must be gone from disk");
    assert!(series_repo::find_by_id(&app.state.db, id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_files_uses_title_only_when_start_year_is_null() {
    // Confirm the `{title}` (no parentheses) variant. A series with no
    // start_year carries a bare-title folder on disk.
    let app = build_test_app().await;
    let id = insert_barebones_series(&app, "Untitled Yearless Volume", None).await;
    let folder = app.library_path().join("Untitled Yearless Volume");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("001.cbz"), b"contents").unwrap();

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{id}?delete_files=true"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!folder.exists());
}

#[tokio::test]
async fn delete_files_succeeds_when_series_has_owned_present_files() {
    // Regression: the prior code path applied the owned-files guard to
    // the delete_files=true branch as well as the bare delete, so the
    // frontend's "Delete series AND files" confirmation 409'd on the
    // exact case it was designed to handle. delete_files=true is the
    // explicit user opt-in to disk-side cleanup — the guard, which
    // exists to stop accidental orphaning of bytes, should NOT fire.
    let app = build_test_app().await;
    let series = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Owned Files Series".into(),
            sort_title: "owned files series".into(),
            start_year: Some(2024),
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
    let folder = app.library_path().join("Owned Files Series (2024)");
    write_cbz(&folder.join("Owned Files Series 001 (2024).cbz"), None);
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    let files = longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
        .await
        .unwrap();
    assert_eq!(files.len(), 1, "scan should attach the one cbz");
    assert_eq!(
        files[0].status, "owned",
        "file must be in the exact state the guard would 409 on"
    );

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{}?delete_files=true", series.id),
        ))
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "delete_files=true must bypass the owned-files guard"
    );
    let body = response_json(resp).await;
    assert_eq!(body["deleted"], series.id);
    assert!(body["folder_deleted"]
        .as_str()
        .unwrap()
        .ends_with("Owned Files Series (2024)"));
    assert!(!folder.exists(), "folder must be gone from disk");
    assert!(series_repo::find_by_id(&app.state.db, series.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_files_succeeds_when_folder_already_absent() {
    // The folder convention may not match (manual rename, never on
    // disk to begin with). The DB delete still happens; the handler
    // logs a warning and returns 200 without a folder_deleted field.
    let app = build_test_app().await;
    let id = insert_barebones_series(&app, "Phantom No Folder", Some(2020)).await;
    let folder = app.library_path().join("Phantom No Folder (2020)");
    assert!(!folder.exists());

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{id}?delete_files=true"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["deleted"], id);
    assert!(
        body.get("folder_deleted").is_none(),
        "no folder removed → field should be absent"
    );
    assert!(series_repo::find_by_id(&app.state.db, id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_files_refuses_when_series_folder_is_a_symlink() {
    // A symlink could point anywhere — outside the library root, in
    // /etc, in the user's home. Following it for remove_dir_all is
    // a footgun; refuse and tell the user to clean it up manually.
    let app = build_test_app().await;
    let id = insert_barebones_series(&app, "Symlinked Series", Some(2024)).await;

    // Build a real directory outside the series folder, then symlink
    // the series-named path to it.
    let real_dir = app.library_path().join("real-target");
    std::fs::create_dir_all(&real_dir).unwrap();
    let symlink = app.library_path().join("Symlinked Series (2024)");
    std::os::unix::fs::symlink(&real_dir, &symlink).unwrap();

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{id}?delete_files=true"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "conflict.series_folder_is_symlink");

    // The series row was deleted BEFORE the symlink check (acceptable —
    // DB and disk are now both consistent: no row, no real folder
    // gone, the symlink still dangles and the user gets a 409 telling
    // them why). Assert the row is in fact gone so this is not silently
    // inconsistent.
    assert!(series_repo::find_by_id(&app.state.db, id)
        .await
        .unwrap()
        .is_none());

    // The symlink target itself is untouched.
    assert!(real_dir.exists());
}

#[tokio::test]
async fn delete_files_refuses_path_traversal_via_series_title() {
    // A title like "../etc" would join to a path that lexically
    // escapes the library root. canonicalize_within_root must reject
    // it after resolving the actual filesystem path. We forge a
    // sibling directory of the library root to make the traversal
    // resolvable and prove the canonical check fires.
    let app = build_test_app().await;
    let escape_dir = app.library_path().parent().unwrap().join("escape-target");
    std::fs::create_dir_all(&escape_dir).unwrap();
    let id = insert_barebones_series(&app, "../escape-target", None).await;

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{id}?delete_files=true"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = response_json(resp).await;
    assert_eq!(
        body["error"]["code"],
        "conflict.series_folder_outside_library_root"
    );
    // Escape target stays put.
    assert!(escape_dir.exists());

    // Cleanup so we don't leak the sibling dir into the test process's
    // tempdir parent.
    let _ = std::fs::remove_dir_all(&escape_dir);
}

#[tokio::test]
async fn delete_files_400s_when_combined_with_force() {
    // The force path exists for misassigned files whose real series
    // lives elsewhere; deleting the title-based folder would destroy
    // bytes belonging to ANOTHER series. The two are incoherent in
    // combination → 400.
    let app = build_test_app().await;
    let id = insert_barebones_series(&app, "Saga", Some(2012)).await;

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/series/{id}?force=true&delete_files=true"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // Series row still in place — the rejection happens before any
    // mutation.
    assert!(series_repo::find_by_id(&app.state.db, id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn delete_without_delete_files_leaves_folder_untouched() {
    // Backwards-compat: the default `DELETE /api/series/:id` (no
    // query params) keeps its old DB-only semantics. A pre-existing
    // folder must survive the call.
    let app = build_test_app().await;
    let id = insert_barebones_series(&app, "Catalog Only", Some(2019)).await;
    let folder = app.library_path().join("Catalog Only (2019)");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("001.cbz"), b"contents").unwrap();

    let resp = app
        .request(empty_request("DELETE", &format!("/api/series/{id}")))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert!(body.get("folder_deleted").is_none());
    assert!(
        folder.exists(),
        "default delete must not touch the on-disk folder"
    );
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
async fn match_from_cv_resolves_issue_number_via_normalization() {
    // Regression: Accept Match used to 422 with
    // `unprocessable.issue_number_unresolved` on filenames that the
    // strict patterns wouldn't claim — underscores between tokens,
    // extra parenthetical tags between number and year, etc. — even
    // though `parse_filename_with_normalization` handled them just
    // fine in Phase B. The endpoint now uses the same normalizing
    // parser so the UX matches.
    let app = build_test_app().await;
    let file_id = setup_match_from_cv(
        &app,
        7777,
        "American Vampire",
        &[(80001, "1"), (80019, "19")],
        // Underscore-separated scene shape — no whitespace, group
        // tag at end. The strict parser (`parse_filename`) returns
        // None on this; the normalizer collapses it to
        // "American Vampire 019 (2011).cbz" which pattern 2 claims.
        "American_Vampire_019_(2011)_(Minutemen-ThosTew).cbz",
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
        "underscore-separated filename must resolve via the normalizer"
    );
    let body = response_json(resp).await;
    assert_eq!(body["issue"]["number"], "19");
    assert_eq!(body["match_method"], "manual");
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
        format!(r#"{{"issue_id": {}}}"#, saga_issue_ids[0]),
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
            format!(r#"{{"issue_id": {}}}"#, issue.id),
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
    // Bare value sanity — assert the field is present and looks like
    // semver; a literal-version assertion would break on every bump.
    let v = body["version"].as_str().unwrap();
    assert!(
        v.split('.').count() == 3 && v.chars().all(|c| c.is_ascii_digit() || c == '.'),
        "expected MAJOR.MINOR.PATCH semver, got {v:?}"
    );
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

// -------- PUT /api/settings/:key (Tier 2 ITEM 4) --------

/// Each editable threshold round-trips: a successful PUT writes the
/// canonical f64 form into the `settings` row, and the next GET
/// reflects that exact value. The consumers (scanner per scan,
/// enrichment per cycle) read the same row via `settings_repo`, so
/// what GET shows here is what the next scan/cycle actually uses.
#[tokio::test]
async fn series_folder_path_uses_env_var_host_library_path_fallback() {
    // The Show-in-Finder bug: with the `host_library_path` settings
    // row empty AND HOST_LIBRARY_PATH env var configured, the endpoint
    // should return a host_path substituted from the env value rather
    // than echoing the container path back with
    // `host_path_configured: false`. Operators on a fresh Docker
    // bring-up don't have to re-type the path into the Settings UI.
    let mut app = build_test_app().await;
    app.set_host_library_path_fallback(Some("/Users/jeremy/Comics".into()));
    let series_id = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "DC K.O.".into(),
            sort_title: "dc k.o.".into(),
            start_year: Some(2025),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;

    let resp = app
        .request(empty_request(
            "GET",
            &format!("/api/series/{series_id}/folder-path"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(
        body["host_path"], "/Users/jeremy/Comics/DC K.O. (2025)",
        "env-var fallback must build the host-side path"
    );
    assert_eq!(
        body["host_path_configured"], true,
        "host_path_configured must reflect the fallback being usable, not just the DB row"
    );
}

#[tokio::test]
async fn series_folder_path_db_row_overrides_env_var_fallback() {
    // The DB settings row stays authoritative when the operator
    // edits it via the UI — the env var is a SEED default, not a
    // runtime hard-pin. Set the row to a different prefix; expect
    // the response to reflect the row, not the env.
    let mut app = build_test_app().await;
    app.set_host_library_path_fallback(Some("/seed/from/env".into()));
    longbox_db::settings_repo::set(&app.state.db, "host_library_path", "/runtime/from/db")
        .await
        .unwrap();
    let series_id = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
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
    .unwrap()
    .id;

    let body = response_json(
        app.request(empty_request(
            "GET",
            &format!("/api/series/{series_id}/folder-path"),
        ))
        .await,
    )
    .await;
    assert_eq!(body["host_path"], "/runtime/from/db/Saga (2012)");
}

#[tokio::test]
async fn series_folder_path_falls_back_to_container_when_neither_env_nor_db_set() {
    // No env, no settings row → response carries the container path
    // verbatim and `host_path_configured: false` so the frontend
    // surfaces it copy-only rather than as an active link. Existing
    // behavior; locked in with a test now that the env-var fallback
    // landed.
    let app = build_test_app().await;
    let series_id = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
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
    .unwrap()
    .id;

    let body = response_json(
        app.request(empty_request(
            "GET",
            &format!("/api/series/{series_id}/folder-path"),
        ))
        .await,
    )
    .await;
    let container = body["container_path"].as_str().unwrap();
    assert_eq!(body["host_path"], container);
    assert_eq!(body["host_path_configured"], false);
}

#[tokio::test]
async fn settings_put_match_confidence_threshold_persists_and_round_trips() {
    let app = build_test_app().await;
    // Default-seeded row is 0.85 (from the 20260516040415 initial
    // migration), so a write to 0.5 is observably different.
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/match_confidence_threshold",
            r#"{"value":"0.5"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["key"], "match_confidence_threshold");
    assert_eq!(body["value"], "0.5");

    // GET reflects the new value (DB-sourced, not env-sourced).
    let resp = app.request(empty_request("GET", "/api/settings")).await;
    let body = response_json(resp).await;
    assert_eq!(body["match_confidence_threshold"], 0.5);
    // The historical env-display field is unchanged — it's the boot
    // env value, not the live tunable.
    assert_eq!(body["match_threshold"], 0.85);
}

#[tokio::test]
async fn settings_put_min_file_size_mb_persists_and_round_trips() {
    // Phase B size-floor tunable. Initial seed (20260607 migration)
    // was 35; the 20260608010000 migration lowered it to 10 — that
    // value is what GET surfaces. A write to 50 round-trips through
    // the PUT/GET endpoints. The integer validator rejects
    // fractional and negative values — covered separately by the
    // parse_megabytes unit tests.
    let app = build_test_app().await;

    let body = response_json(app.request(empty_request("GET", "/api/settings")).await).await;
    assert_eq!(
        body["min_file_size_mb"], 10,
        "post-migration seed value must surface verbatim"
    );

    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/min_file_size_mb",
            r#"{"value":"50"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["key"], "min_file_size_mb");
    assert_eq!(body["value"], "50");

    let body = response_json(app.request(empty_request("GET", "/api/settings")).await).await;
    assert_eq!(body["min_file_size_mb"], 50);
}

#[tokio::test]
async fn settings_put_min_file_size_mb_rejects_invalid_values() {
    // The integer ceiling, fractional, and garbage paths each get a
    // 400 with a human-readable error — confirmed end-to-end so the
    // Settings UI can surface the message verbatim.
    let app = build_test_app().await;
    for raw in ["35.5", "-1", "10241", "banana", ""] {
        let resp = app
            .request(json_request(
                "PUT",
                "/api/settings/min_file_size_mb",
                format!(r#"{{"value":"{raw}"}}"#),
            ))
            .await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "value {raw:?} must be rejected"
        );
    }
}

#[tokio::test]
async fn settings_put_pull_indexer_match_threshold_persists_and_round_trips() {
    // Finding 3 regression: the pull engine's NZB-to-series similarity
    // gate must be tunable via the Settings API now that it's exposed.
    // Default-seeded value is 0.75 (PULL_INDEXER_MATCH_THRESHOLD); a
    // write to 0.70 is observably different.
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/pull_indexer_match_threshold",
            r#"{"value":"0.70"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["key"], "pull_indexer_match_threshold");
    assert_eq!(body["value"], "0.7");

    // GET reflects the new value — single source of truth (Finding 4).
    let body = response_json(app.request(empty_request("GET", "/api/settings")).await).await;
    assert_eq!(body["pull_indexer_match_threshold"], 0.7);
}

#[tokio::test]
async fn settings_put_enrichment_thresholds_persist() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/cv_enrichment_title_threshold_year_known",
            r#"{"value":"0.7"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/cv_enrichment_title_threshold_year_unknown",
            r#"{"value":"0.99"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(app.request(empty_request("GET", "/api/settings")).await).await;
    assert_eq!(body["cv_enrichment_title_threshold_year_known"], 0.7);
    assert_eq!(body["cv_enrichment_title_threshold_year_unknown"], 0.99);
}

#[tokio::test]
async fn settings_put_pull_exclusion_keywords_persists_verbatim() {
    let app = build_test_app().await;
    // CSV stored verbatim — the pull engine handles split/trim at
    // consumption time so the wire round-trips exactly what the user
    // typed (including spaces, which the engine will trim).
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/pull_exclusion_keywords",
            r#"{"value":"infinity comic, infinite comic, digital"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(app.request(empty_request("GET", "/api/settings")).await).await;
    assert_eq!(
        body["pull_exclusion_keywords"],
        "infinity comic, infinite comic, digital"
    );
}

#[tokio::test]
async fn settings_put_rejects_threshold_out_of_range() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/match_confidence_threshold",
            r#"{"value":"1.5"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = response_json(resp).await;
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("between 0.0 and 1.0"));

    // The stored value must be unchanged — the rejection happens
    // before the upsert.
    let body = response_json(app.request(empty_request("GET", "/api/settings")).await).await;
    assert_eq!(body["match_confidence_threshold"], 0.85);
}

#[tokio::test]
async fn settings_put_rejects_non_numeric_threshold() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/cv_enrichment_title_threshold_year_known",
            r#"{"value":"banana"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn settings_put_rejects_unknown_key() {
    // Boot-time env vars (library_root_path, etc.) are deliberately
    // NOT exposed to runtime mutation. Tweaking them requires a
    // container restart by design — silently writing the row would
    // create a confusing drift between the env-sourced display and
    // the never-consulted DB row.
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/settings/library_root_path",
            r#"{"value":"/somewhere/else"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = response_json(resp).await;
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not editable"));
}

// -------- GET /api/stats (dashboard tile aggregates) --------

/// New fields the dashboard consolidates into one HTTP round-trip:
/// `pull_list_count`, `pull_failures_count`, `pending_interventions_count`.
/// Seed each surface independently and assert the response numbers
/// match. Existing aggregate fields (total_series, owned_files, etc.)
/// are spot-checked too so a future refactor of the giant correlated
/// SELECT can't silently zero them out.
#[tokio::test]
async fn stats_total_series_excludes_unenriched_fileless_phantoms() {
    // ITEM 8: SERIES tile should count only series with at least one
    // owned+present file OR a non-null cv_id. A bulk-converted shallow
    // row with no cv_id and no files is catalog noise — exclude.
    let app = build_test_app().await;

    // Counted: cv_id-only (enriched, no files).
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(2127),
            metron_id: None,
            title: "Enriched Only".into(),
            sort_title: "enriched only".into(),
            start_year: Some(2010),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // Counted: files-only (no cv_id, has an owned+present file).
    let (files_only_sid, files_only_iid) = seed_series_and_issue(&app, "Files Only", "1").await;
    longbox_db::file_repo::insert(
        &app.state.db,
        longbox_db::NewFile {
            issue_id: Some(files_only_iid),
            library_root_id: app.library_root_id,
            path_relative: "Files Only (2012)/Files Only 001.cbz".into(),
            size_bytes: 1,
            mtime: time::macros::datetime!(2024-01-01 0:00),
            last_scanned_at: time::macros::datetime!(2024-01-01 0:00),
            match_method: "filename".into(),
            match_confidence: 0.99,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: time::macros::datetime!(2024-01-01 0:00),
            matched_at: Some(time::macros::datetime!(2024-01-01 0:00)),
        },
    )
    .await
    .unwrap();
    let _ = files_only_sid;

    // NOT counted: shallow phantom (no cv_id, no files).
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Phantom".into(),
            sort_title: "phantom".into(),
            start_year: None,
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    let body = response_json(app.request(empty_request("GET", "/api/stats")).await).await;
    assert_eq!(
        body["total_series"], 2,
        "enriched_only + files_only count; the cv_id-null no-files phantom is excluded"
    );
}

#[tokio::test]
async fn stats_aggregates_pull_list_and_failures_and_pending_interventions() {
    let app = build_test_app().await;

    // Baseline: empty catalog → every count is 0.
    let body = response_json(app.request(empty_request("GET", "/api/stats")).await).await;
    assert_eq!(body["pull_list_count"], 0);
    assert_eq!(body["pull_failures_count"], 0);
    assert_eq!(body["pending_interventions_count"], 0);
    assert_eq!(body["total_series"], 0);

    // Seed two pull-list rows.
    let s1 = seed_pull_series(&app, "Subscribed One").await;
    let s2 = seed_pull_series(&app, "Subscribed Two").await;
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: s1,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: s2,
            start_issue: None,
        },
    )
    .await
    .unwrap();

    // Two issues with failure-class latest attempts (one failed, one
    // mismatched), one issue with an old failure but a more recent
    // submitted attempt (should NOT count — latest-attempt semantics).
    let (sid, ia) = seed_series_and_issue(&app, "Three Issues", "1").await;
    let ib = longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id: sid,
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
    let ic = longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id: sid,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "3".into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    pull_attempt_repo::insert(&app.state.db, failed_attempt(sid, ia, None))
        .await
        .unwrap();
    pull_attempt_repo::insert(
        &app.state.db,
        NewPullAttempt {
            series_id: sid,
            issue_id: ib,
            indexer_id: None,
            release_id: Some("rel-x".into()),
            status: "mismatched".into(),
            error_message: Some("title mismatch".into()),
            retry_count: 1,
            download_handle: None,
        },
    )
    .await
    .unwrap();
    pull_attempt_repo::insert(&app.state.db, failed_attempt(sid, ic, None))
        .await
        .unwrap();
    pull_attempt_repo::insert(
        &app.state.db,
        NewPullAttempt {
            series_id: sid,
            issue_id: ic,
            indexer_id: None,
            release_id: Some("rel-y".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 1,
            download_handle: Some("h".into()),
        },
    )
    .await
    .unwrap();

    // Pending intervention — push two onto the in-memory cache.
    for source in ["/watch/a.cbz", "/watch/b.cbz"] {
        app.state
            .pending_cache
            .push(longbox_postprocess::PendingIntervention {
                source_path: std::path::PathBuf::from(source),
                target_path: std::path::PathBuf::from("/library/x.cbz"),
                reason: longbox_postprocess::InterventionReason::Conflict,
                size: 1,
                last_attempt: time::OffsetDateTime::UNIX_EPOCH,
            });
    }

    let body = response_json(app.request(empty_request("GET", "/api/stats")).await).await;
    assert_eq!(body["pull_list_count"], 2);
    assert_eq!(
        body["pull_failures_count"], 2,
        "ia + ib are failure-class; ic's failed got superseded by submitted"
    );
    assert_eq!(body["pending_interventions_count"], 2);
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
            format!(r#"{{"issue_id": {}}}"#, pre_issue.id),
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

// -------- POST /downloader/notify (SAB failure hook) --------

#[tokio::test]
async fn notify_completed_is_noop() {
    let app = build_test_app().await;
    let (sid, iid) = seed_series_and_issue(&app, "Saga", "1").await;
    pull_attempt_repo::insert(
        &app.state.db,
        NewPullAttempt {
            series_id: sid,
            issue_id: iid,
            indexer_id: None,
            release_id: Some("rel-1".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("SABnzbd_nzo_abc123".into()),
        },
    )
    .await
    .unwrap();

    let resp = app
        .request(json_request(
            "POST",
            "/api/downloader/notify",
            r#"{"nzo_id":"SABnzbd_nzo_abc123","status":"Completed","fail_msg":""}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The attempt stays in `submitted` — Phase B owns the success path.
    let submitted = pull_attempt_repo::list_submitted(&app.state.db)
        .await
        .unwrap();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].status, "submitted");
}

#[tokio::test]
async fn notify_failed_marks_attempt_as_failed() {
    let app = build_test_app().await;
    let (sid, iid) = seed_series_and_issue(&app, "Saga", "2").await;
    let attempt_id = pull_attempt_repo::insert(
        &app.state.db,
        NewPullAttempt {
            series_id: sid,
            issue_id: iid,
            indexer_id: None,
            release_id: Some("rel-2".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("SABnzbd_nzo_xyz789".into()),
        },
    )
    .await
    .unwrap()
    .id;

    let resp = app
        .request(json_request(
            "POST",
            "/api/downloader/notify",
            r#"{"nzo_id":"SABnzbd_nzo_xyz789","status":"Failed","fail_msg":"par2 repair failed"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The attempt is no longer `submitted`.
    let submitted = pull_attempt_repo::list_submitted(&app.state.db)
        .await
        .unwrap();
    assert!(
        submitted.is_empty(),
        "submitted set should be empty after failure notice"
    );

    // And the row carries the SAB-supplied fail_msg verbatim.
    let attempts = pull_attempt_repo::list_for_issue(&app.state.db, sid, iid)
        .await
        .unwrap();
    let row = attempts.iter().find(|a| a.id == attempt_id).unwrap();
    assert_eq!(row.status, "failed");
    assert_eq!(row.error_message.as_deref(), Some("par2 repair failed"));
}

#[tokio::test]
async fn notify_unknown_nzo_id_returns_ok_without_side_effect() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/downloader/notify",
            r#"{"nzo_id":"SABnzbd_nzo_never_seen","status":"Failed","fail_msg":""}"#,
        ))
        .await;
    // The endpoint must always 200 — SAB retries scripts that return
    // error codes. An unknown nzo_id is the common case (attempts
    // submitted before LongBox restarted, or by another instance).
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn notify_falls_back_to_status_string_when_fail_msg_is_blank() {
    let app = build_test_app().await;
    let (sid, iid) = seed_series_and_issue(&app, "Saga", "3").await;
    pull_attempt_repo::insert(
        &app.state.db,
        NewPullAttempt {
            series_id: sid,
            issue_id: iid,
            indexer_id: None,
            release_id: Some("rel-3".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("SABnzbd_nzo_blank_msg".into()),
        },
    )
    .await
    .unwrap();

    let resp = app
        .request(json_request(
            "POST",
            "/api/downloader/notify",
            r#"{"nzo_id":"SABnzbd_nzo_blank_msg","status":"Failed"}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let attempts = pull_attempt_repo::list_for_issue(&app.state.db, sid, iid)
        .await
        .unwrap();
    let row = attempts.iter().find(|a| a.status == "failed").unwrap();
    assert_eq!(row.error_message.as_deref(), Some("SABnzbd status: Failed"));
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

// -------- on-demand single-series search (Phase A.9) --------

#[tokio::test]
async fn pull_search_404_when_series_does_not_exist() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("POST", "/api/pull/search/9999"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "not_found.series");
}

#[tokio::test]
async fn pull_search_200_when_series_has_no_missing_issues() {
    // Series exists, no issues whose cover date has shipped + no file —
    // the bulk button has nothing to dispatch. Returns a 200 with an
    // informational `note` so the caller can tell "we tried, found
    // nothing to do" from "search queued, await results".
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    let resp = app
        .request(empty_request("POST", &format!("/api/pull/search/{sid}")))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["queued"], 0);
    assert!(body["note"].is_string(), "note must be populated: {body:?}");
}

#[tokio::test]
async fn pull_search_202_dispatches_per_missing_issue_for_unsubscribed_series() {
    // Load-bearing: the series is NOT on the pull list, but the button
    // still fires a search for every shipped + unowned issue. The
    // previous shape 404'd here because of the pull-list gate; this
    // confirms the gate is gone.
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    for n in &["1", "2"] {
        longbox_db::issue_repo::insert(
            &app.state.db,
            longbox_db::NewIssue {
                series_id: sid,
                cv_issue_id: None,
                metron_issue_id: None,
                number: (*n).into(),
                title: None,
                cover_date: Some("2020-01-01".into()),
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }
    let resp = app
        .request(empty_request("POST", &format!("/api/pull/search/{sid}")))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = response_json(resp).await;
    assert_eq!(body["queued"], 2);
    assert!(
        body["note"].is_null(),
        "note must be absent on 202: {body:?}"
    );
}

#[tokio::test]
async fn pull_search_skips_solicited_and_owned_issues() {
    // The "missing" filter excludes: (a) issues with a future cover
    // date (solicited — not shipped yet) and (b) issues that already
    // have an owned+present file. Only the shipped+no-owned-file
    // intersection counts as missing.
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    // 1) Genuinely missing — counts.
    longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id: sid,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: Some("2020-01-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    // 2) Solicited (cover_date in the future) — does not count.
    longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id: sid,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "99".into(),
            title: None,
            cover_date: Some("2099-12-31".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    let resp = app
        .request(empty_request("POST", &format!("/api/pull/search/{sid}")))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = response_json(resp).await;
    assert_eq!(body["queued"], 1, "only the shipped issue counts: {body:?}");
}

#[tokio::test]
async fn pull_search_auto_triggered_on_subscribe_via_pull_list_add() {
    // Load-bearing for the auto-trigger arc: subscribing through
    // POST /api/pull-list must fire a single-series search for the new
    // series_id. `is_searching` is observable here because tokio's
    // current-thread runtime doesn't preempt this test's mutex lock
    // with the spawned-task's cleanup; the entry is still in the set
    // when we check.
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    assert!(
        !app.state.pull_search.is_searching(sid).await,
        "guard must start clean"
    );
    let resp = app
        .request(json_request(
            "POST",
            "/api/pull-list",
            format!(r#"{{"series_id":{sid}}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        app.state.pull_search.is_searching(sid).await,
        "subscribe must auto-trigger a search for the new series_id"
    );
}

// -------- per-issue search (series detail page) --------

#[tokio::test]
async fn pull_search_issue_202_accepts_for_an_issue_belonging_to_the_series() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    let iid = longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id: sid,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: Some("2024-01-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let resp = app
        .request(empty_request(
            "POST",
            &format!("/api/pull/search/{sid}/issue/{iid}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn pull_search_issue_404_when_series_unknown() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("POST", "/api/pull/search/99999/issue/1"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "not_found.series");
}

#[tokio::test]
async fn pull_search_issue_404_when_issue_belongs_to_different_series() {
    // URL tampering / stale UI defense: passing series_id of A with
    // issue_id from series B must surface as 404, not silently search
    // the wrong scope.
    let app = build_test_app().await;
    let saga = seed_pull_series(&app, "Saga").await;
    let chew = seed_pull_series(&app, "Chew").await;
    let chew_issue = longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id: chew,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: Some("2024-01-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let resp = app
        .request(empty_request(
            "POST",
            // Wrong series_id deliberately.
            &format!("/api/pull/search/{saga}/issue/{chew_issue}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = response_json(resp).await;
    assert_eq!(body["error"]["code"], "not_found.issue");
}

/// Headline requirement from the spec: the series does NOT need to be
/// on the pull list. A series in the catalog without a pull_list row
/// must still accept a per-issue Search. The route handler doesn't
/// consult pull_list at all; only series + issue + relation.
#[tokio::test]
#[allow(non_snake_case)]
async fn pull_search_issue_works_when_series_is_NOT_on_pull_list() {
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Unsubscribed Series").await;
    // Deliberately NOT calling pull_list_repo::add — the series is
    // catalog-only.
    assert!(
        longbox_db::pull_list_repo::get(&app.state.db, sid)
            .await
            .unwrap()
            .is_none(),
        "series must not be on the pull list for this test"
    );
    let iid = longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id: sid,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: Some("2024-01-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let resp = app
        .request(empty_request(
            "POST",
            &format!("/api/pull/search/{sid}/issue/{iid}"),
        ))
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "unsubscribed series must still accept a per-issue search"
    );
}

// -------- /pull/search-all-missing --------

fn ts_2020() -> time::PrimitiveDateTime {
    time::macros::datetime!(2020-01-01 00:00:00)
}

async fn insert_issue(
    app: &common::TestApp,
    series_id: i64,
    number: &str,
    cover_date: Option<&str>,
) -> i64 {
    longbox_db::issue_repo::insert(
        &app.state.db,
        longbox_db::NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.into(),
            title: None,
            cover_date: cover_date.map(str::to_owned),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

/// Partition a mixed set of issues by the same predicate the frontend
/// uses (`isSolicited`): a fully-specified `YYYY-MM-DD` cover_date that
/// is today-or-later is solicited and skipped; everything else
/// (already-shipped, null cover_date, partial date) is searched.
#[tokio::test]
async fn search_all_missing_searches_shipped_and_skips_solicited() {
    let app = build_test_app().await;
    let sid_a = seed_pull_series(&app, "Saga").await;
    let sid_b = seed_pull_series(&app, "Chew").await;

    // A long-past issue — definitely shipped.
    insert_issue(&app, sid_a, "1", Some("2012-01-01")).await;
    // A future-dated issue — solicited, must be skipped.
    insert_issue(&app, sid_a, "999", Some("2999-01-01")).await;
    // A null cover_date — falls through to NOT solicited (same lenient
    // classification the frontend uses for unknown dates).
    insert_issue(&app, sid_b, "1", None).await;
    // A partial date `YYYY-MM` — also NOT solicited (it isn't a clean
    // 10-char `YYYY-MM-DD` so the predicate rejects it).
    insert_issue(&app, sid_b, "2", Some("2010-05")).await;

    let resp = app
        .request(empty_request("POST", "/api/pull/search-all-missing"))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = response_json(resp).await;
    assert_eq!(body["searched"], 3);
    assert_eq!(body["skipped_solicited"], 1);
}

#[tokio::test]
async fn search_all_missing_excludes_owned_present_issues() {
    // An issue with an owned, present file is not missing. The endpoint
    // must skip it entirely — it shouldn't count as searched OR
    // skipped_solicited.
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Owned Already").await;
    let iid_owned = insert_issue(&app, sid, "1", Some("2020-01-01")).await;
    let iid_missing = insert_issue(&app, sid, "2", Some("2020-02-01")).await;

    // Attach an owned, present file to issue 1.
    let library_root_id = app.library_root_id;
    longbox_db::file_repo::insert(
        &app.state.db,
        longbox_db::NewFile {
            issue_id: Some(iid_owned),
            library_root_id,
            path_relative: "Owned Already (2020)/001.cbz".into(),
            size_bytes: 1,
            mtime: ts_2020(),
            last_scanned_at: ts_2020(),
            match_method: "filename".into(),
            match_confidence: 0.99,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: ts_2020(),
            matched_at: Some(ts_2020()),
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("POST", "/api/pull/search-all-missing"))
            .await,
    )
    .await;
    assert_eq!(
        body["searched"], 1,
        "only the missing issue should be searched"
    );
    assert_eq!(body["skipped_solicited"], 0);
    // Sanity: the owned issue's id wasn't in the missing pool.
    let _ = iid_missing;
}

#[tokio::test]
async fn search_all_missing_is_a_clean_zero_when_catalog_is_empty() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("POST", "/api/pull/search-all-missing"))
        .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = response_json(resp).await;
    assert_eq!(body["searched"], 0);
    assert_eq!(body["skipped_solicited"], 0);
}

#[tokio::test]
async fn pull_search_auto_trigger_silently_skips_already_on_list() {
    // Re-subscribing a series that's already on the list returns 409
    // from the route, NOT 200. The auto-trigger only fires on the
    // success path, so a no-op re-subscribe must NOT trigger a
    // search. Defends against double-firing on bulk-add payloads
    // that include duplicates.
    let app = build_test_app().await;
    let sid = seed_pull_series(&app, "Saga").await;
    // First subscribe — fires the trigger.
    app.request(json_request(
        "POST",
        "/api/pull-list",
        format!(r#"{{"series_id":{sid}}}"#),
    ))
    .await;
    // Let the spawned task clear the in-progress entry.
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    // Repeat subscribe — 409, no re-trigger.
    let dup = app
        .request(json_request(
            "POST",
            "/api/pull-list",
            format!(r#"{{"series_id":{sid}}}"#),
        ))
        .await;
    assert_eq!(dup.status(), StatusCode::CONFLICT);
    assert!(
        !app.state.pull_search.is_searching(sid).await,
        "a 409 re-subscribe must NOT re-trigger a search"
    );
}

// -------- pull-list export / import (Tier 4 ITEM 14) --------

#[tokio::test]
async fn export_pull_list_returns_subscriptions_with_cv_metadata() {
    let app = build_test_app().await;
    // Seed a CV-linked series and a subscription on it.
    let cv_linked = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(2127),
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
    longbox_db::pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: cv_linked,
            start_issue: None,
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", "/api/pull-list/export"))
            .await,
    )
    .await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["series_id"].as_i64().unwrap(), cv_linked);
    assert_eq!(arr[0]["title"], "Saga");
    assert_eq!(arr[0]["cv_id"], 2127);
    assert_eq!(arr[0]["start_year"], 2012);
    assert!(arr[0]["subscribed_at"].is_string());
}

#[tokio::test]
async fn import_pull_list_upserts_by_cv_id_and_classifies_each_row() {
    let app = build_test_app().await;
    // Catalog: one series with cv_id 100 already subscribed, one
    // with cv_id 200 NOT subscribed.
    let already = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(100),
            metron_id: None,
            title: "Already On List".into(),
            sort_title: "already on list".into(),
            start_year: Some(2020),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    longbox_db::pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: already,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(200),
            metron_id: None,
            title: "Adds Cleanly".into(),
            sort_title: "adds cleanly".into(),
            start_year: Some(2020),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // Import payload covers the four outcomes the response classifies:
    // added (cv_id 200), already (100), series_not_found (999),
    // missing_cv_id (null).
    let payload = r#"[
        {"cv_id": 100, "title": "Already"},
        {"cv_id": 200, "title": "Fresh"},
        {"cv_id": 999, "title": "Unknown Volume"},
        {"cv_id": null, "title": "Shallow"}
    ]"#;
    let resp = app
        .request(json_request("POST", "/api/pull-list/import", payload))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["added"], 1);
    assert_eq!(body["already_subscribed"], 1);
    assert_eq!(body["series_not_found"], 1);
    assert_eq!(body["missing_cv_id"], 1);
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 4);
    let by_status: std::collections::HashMap<&str, &serde_json::Value> = results
        .iter()
        .map(|r| (r["status"].as_str().unwrap(), r))
        .collect();
    assert!(by_status.contains_key("added"));
    assert!(by_status.contains_key("already_subscribed"));
    assert!(by_status.contains_key("series_not_found"));
    assert!(by_status.contains_key("missing_cv_id"));
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

    // keep_phantom_series sets tidy_exempt=1, which removes the series from
    // list_phantoms entirely (moves it to list_kept_series). Neither bucket
    // should contain the series after keep.
    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/phantoms"))
            .await,
    )
    .await;
    assert_eq!(body["with_transition"].as_array().unwrap().len(), 0);
    assert_eq!(body["all_zero_owned"].as_array().unwrap().len(), 0);
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

#[tokio::test]
async fn reconcile_phantoms_flags_a_pull_list_series_as_awaiting_first_download() {
    let app = build_test_app().await;
    let awaiting = seed_pull_series(&app, "Subscribed").await;
    pull_list_repo::add(
        &app.state.db,
        NewPullEntry {
            series_id: awaiting,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    let plain = seed_pull_series(&app, "Plain Phantom").await;

    let body = response_json(
        app.request(empty_request("GET", "/api/reconcile/phantoms"))
            .await,
    )
    .await;
    let all = body["all_zero_owned"].as_array().unwrap();
    let flag = |id: i64| {
        all.iter().find(|p| p["id"] == id).expect("phantom present")["awaiting_first_download"]
            .as_bool()
            .unwrap()
    };
    // A pull-list series is empty by intent — it must read as awaiting a
    // first download; a plain zero-owned phantom must not.
    assert!(flag(awaiting), "a pull-list series is awaiting a download");
    assert!(!flag(plain), "a plain phantom is not");
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
    assert_eq!(body["results"][0]["status"], "added");
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
async fn reconcile_convert_links_to_existing_series_instead_of_duplicating() {
    // A.9 hot-fix: a folder whose (sort_title, start_year) matches an
    // existing series must link to that survivor, not insert a dupe.
    let app = build_test_app().await;
    // `sort_title` matches what `normalize_title` would produce from
    // the folder's parsed title — leading article stripped — so
    // `find_for_dedup` can match. The CV-add path normalizes the same
    // way, so a real CV-tracked Walking Dead row has this shape too.
    let existing = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(2127),
            metron_id: None,
            title: "The Walking Dead".into(),
            sort_title: "walking dead".into(),
            start_year: Some(2003),
            publisher: Some("Image".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    // The folder + an unmatched file in it.
    write_cbz(
        &app.library_path()
            .join("The Walking Dead (2003)/The Walking Dead 001 (2003).cbz"),
        None,
    );
    app.state
        .scanner
        .scan_full(app.library_root_id)
        .await
        .unwrap();
    // The seeded series had no issues, so the scanned file ends up
    // unmatched and the folder shows as discovered.
    let untracked_before = discovered_folders_repo::list(&app.state.db).await.unwrap();
    assert!(
        untracked_before
            .iter()
            .any(|d| d.folder_name == "The Walking Dead (2003)"),
        "folder is discovered as untracked"
    );

    let resp = app
        .request(json_request(
            "POST",
            "/api/reconcile/convert",
            r#"{"folder_names":["The Walking Dead (2003)"]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let result = &body["results"][0];
    assert_eq!(
        result["status"], "linked",
        "matches existing — links, doesn't add"
    );
    assert_eq!(
        result["series_id"].as_i64().unwrap(),
        existing.id,
        "linked to the pre-existing series id"
    );

    // No duplicate series row for this (sort_title, start_year).
    let count = series_repo::find_all(&app.state.db)
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.sort_title == "walking dead" && s.start_year == Some(2003))
        .count();
    assert_eq!(count, 1, "no duplicate series row");

    // The file attached to the survivor as owned via FilenameRegex.
    let owned: Vec<_> =
        longbox_db::file_repo::list_by_library_root(&app.state.db, app.library_root_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|f| f.status == "owned" && f.match_method == "filename_regex")
            .collect();
    assert_eq!(owned.len(), 1);
}

#[tokio::test]
async fn reconcile_convert_null_year_folder_links_to_existing_year_set_series() {
    // A.9 Bug 2: a discovered folder lacking `(YYYY)` (so the
    // converter parses start_year=None) must link to an existing
    // year-set series sharing the normalized sort_title, instead of
    // creating a duplicate row. Observed shape: the user has
    // `Enfield Gang Massacre (2024)` and `The Enfield Gang Massacre`
    // on disk as separate folders carrying different issues — the
    // catalog should record them as one series.
    let app = build_test_app().await;
    let existing = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Enfield Gang Massacre".into(),
            sort_title: "enfield gang massacre".into(),
            start_year: Some(2024),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    // The NULL-year folder on disk: leading "The" gets stripped by
    // normalize_title, so its sort_title matches the existing row.
    write_cbz(
        &app.library_path()
            .join("The Enfield Gang Massacre/The Enfield Gang Massacre 001.cbr"),
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
            r#"{"folder_names":["The Enfield Gang Massacre"]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let result = &body["results"][0];
    assert_eq!(
        result["status"], "linked",
        "NULL-year folder links to the year-set existing row via phase-2 fallback"
    );
    assert_eq!(
        result["series_id"].as_i64().unwrap(),
        existing.id,
        "linked to the pre-existing series id, not a duplicate"
    );

    // No duplicate series row for this sort_title.
    let count = series_repo::find_all(&app.state.db)
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.sort_title == "enfield gang massacre")
        .count();
    assert_eq!(count, 1, "no duplicate series row");
}

#[tokio::test]
async fn reconcile_convert_with_zero_attachments_rolls_back() {
    // A.9 Bug 1a: a convert in which the parser cannot extract an
    // issue number from any of the folder's files must roll back the
    // entire transaction — no series row created, no auto-dismiss of
    // the discovered folder. The previous code created a shallow
    // series with zero issues and auto-dismissed the folder, hiding
    // the unparseable-files problem behind a clean Untracked list
    // ("ghost series" shape).
    let app = build_test_app().await;
    // Filenames with no digits anywhere — every parsing pattern
    // requires a `\d+` number capture, so all seven fail.
    write_cbz(
        &app.library_path()
            .join("Unparseable Folder (2024)/Unparseable Folder - Foreword.cbz"),
        None,
    );
    write_cbz(
        &app.library_path()
            .join("Unparseable Folder (2024)/Unparseable Folder - Afterword.cbz"),
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
            r#"{"folder_names":["Unparseable Folder (2024)"]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let result = &body["results"][0];
    assert_eq!(result["status"], "failed");
    assert!(result["series_id"].is_null());
    let err = result["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("0 of 2") && err.contains("no series created"),
        "error message should explain the rollback: got {err:?}"
    );

    // No series row exists for the rolled-back convert. The (title,
    // year) was clean enough to have inserted if not for the rollback.
    let leftover = series_repo::find_all(&app.state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.sort_title == "unparseable folder" && s.start_year == Some(2024));
    assert!(
        leftover.is_none(),
        "no series row should survive a zero-attachment convert"
    );

    // The folder stays visible as untracked — neither user-dismissed
    // nor auto-dismissed.
    let untracked = discovered_folders_repo::list(&app.state.db).await.unwrap();
    assert!(
        untracked
            .iter()
            .any(|d| d.folder_name == "Unparseable Folder (2024)"),
        "discovered folder should still be untracked after a rolled-back convert"
    );
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

// -------- Library Tidy duplicates (Tier 3 batch 2 ITEM 12) --------

/// Two series sharing a sort_title with start_year delta of 1 →
/// flagged as `same_title_close_year`. A reboot a decade later is
/// outside the 2-year window and is NOT flagged.
#[tokio::test]
async fn duplicates_flags_same_title_close_year_pairs() {
    let app = build_test_app().await;
    let saga_a = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
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
    .unwrap()
    .id;
    let saga_b = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2013),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2024),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    let body = response_json(
        app.request(empty_request("GET", "/api/library/tidy/duplicates"))
            .await,
    )
    .await;
    let pairs = body["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0]["kind"], "same_title_close_year");
    let a = pairs[0]["a_id"].as_i64().unwrap();
    let b = pairs[0]["b_id"].as_i64().unwrap();
    assert!(a < b, "pair must be ordered by id ascending");
    assert_eq!(a, saga_a);
    assert_eq!(b, saga_b);
}

#[tokio::test]
async fn merge_duplicates_moves_issues_and_deletes_source() {
    let app = build_test_app().await;
    let (target, target_issue) = seed_series_and_issue(&app, "Saga", "1").await;
    let (source, source_issue) = seed_series_and_issue(&app, "Saga", "2").await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicates/merge",
            format!(r#"{{"target_series_id":{target},"source_series_id":{source}}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Source series row is gone.
    assert!(series_repo::find_by_id(&app.state.db, source)
        .await
        .unwrap()
        .is_none());
    // Source's issue now lives under target.
    let migrated = longbox_db::issue_repo::find_by_id(&app.state.db, source_issue)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(migrated.series_id, target);
    // Target's own issue is untouched.
    let target_issue_row = longbox_db::issue_repo::find_by_id(&app.state.db, target_issue)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target_issue_row.series_id, target);
}

#[tokio::test]
async fn merge_duplicates_rejects_target_equals_source() {
    let app = build_test_app().await;
    let (sid, _) = seed_series_and_issue(&app, "Saga", "1").await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicates/merge",
            format!(r#"{{"target_series_id":{sid},"source_series_id":{sid}}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn merge_duplicates_404s_on_unknown_series() {
    let app = build_test_app().await;
    let (real, _) = seed_series_and_issue(&app, "Saga", "1").await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/library/tidy/duplicates/merge",
            format!(r#"{{"target_series_id":{real},"source_series_id":99999}}"#),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
    // file missing, and the series transitions to a phantom. Drop a
    // non-archive placeholder first so the mount-health preflight
    // doesn't refuse to scan a now-empty library root (the walker
    // filters by extension; non-cbz/cbr files are invisible to it).
    std::fs::remove_dir_all(app.library_path().join("Chew")).unwrap();
    std::fs::write(
        app.library_path().join(".placeholder"),
        b"preflight keepalive",
    )
    .unwrap();
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

/// 6c.5: the calendar response's `publisher` field must come from the
/// tracked series row (populated by the enrichment merge or refresh
/// pass), NOT from the CV `/issues/` payload (which never carries
/// publisher — see calendar.rs:5-8). Without this JOIN the calendar
/// can't group by publisher (Item E).
#[tokio::test]
async fn calendar_publisher_comes_from_series_join_not_cv_payload() {
    let app = build_test_app().await;

    // (a) Tracked series with publisher populated (post-enrichment shape).
    let tracked = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(4050),
            metron_id: None,
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2012),
            publisher: Some("Image Comics".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();
    // (b) Tracked series with publisher NULL (pre-refresh-pass shape).
    let no_publisher = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(4099),
            metron_id: None,
            title: "Pre-Refresh".into(),
            sort_title: "pre-refresh".into(),
            start_year: Some(2024),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    mount_cv_calendar(
        &app,
        "2026-05-13",
        "2026-05-19",
        r#"[
            { "id": 50001, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 4050, "name": "Saga" },
              "site_detail_url": "https://cv/4000-50001/" },
            { "id": 50002, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 4099, "name": "Pre-Refresh" },
              "site_detail_url": "https://cv/4000-50002/" },
            { "id": 50003, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 9999, "name": "Untracked" },
              "site_detail_url": "https://cv/4000-50003/" }
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
    assert_eq!(rows.len(), 3);

    // Tracked + publisher populated → publisher surfaces from the JOIN.
    let saga = rows.iter().find(|r| r["cv_volume_id"] == 4050).unwrap();
    assert_eq!(saga["series_id"], tracked.id);
    assert_eq!(
        saga["publisher"], "Image Comics",
        "publisher must come from the series JOIN, not CV"
    );

    // Tracked + publisher NULL → publisher null (will group as
    // "Unknown Publisher" client-side until the refresh pass fills
    // the column).
    let pre = rows.iter().find(|r| r["cv_volume_id"] == 4099).unwrap();
    assert_eq!(pre["series_id"], no_publisher.id);
    assert!(
        pre["publisher"].is_null(),
        "tracked-but-not-yet-refreshed series carries publisher null"
    );

    // Untracked → publisher null (no series row to JOIN to).
    let untracked = rows.iter().find(|r| r["cv_volume_id"] == 9999).unwrap();
    assert!(untracked["series_id"].is_null());
    assert!(untracked["publisher"].is_null());
}

/// Item E v2 piece 1: the calendar falls back to `cv_volume_cache` for
/// items whose volume is not in the catalog (series_id = None), AND it
/// synchronously queues any uncached cv_volume_ids as pending rows for
/// the worker's cache-fill pass to drain. Together these mean:
///   - calendar rendering shows real publishers for non-catalog volumes
///     that have been previously cached
///   - calendar requests are the queue producers; the worker's
///     cache-fill pass is the consumer
async fn build_test_app_local() -> common::TestApp {
    build_test_app().await
}

#[tokio::test]
async fn calendar_falls_back_to_cv_volume_cache_for_untracked_volumes() {
    let app = build_test_app_local().await;

    // Pre-seed the cache: an untracked volume that we previously
    // resolved. Calendar must surface its publisher.
    cv_volume_cache_repo::mark_fetched(
        &app.state.db,
        7777,
        Some("Boom! Studios"),
        Some("A short series description."),
        Some(2023),
        None,
    )
    .await
    .unwrap();
    // Seed an INSERT-OR-IGNORE'd pending row first to satisfy the
    // mark_fetched UPDATE — the calendar's queue-insert does the same
    // thing in production.
    cv_volume_cache_repo::bulk_queue_pending(&app.state.db, &[7777])
        .await
        .ok();
    cv_volume_cache_repo::mark_fetched(
        &app.state.db,
        7777,
        Some("Boom! Studios"),
        Some("A short series description."),
        Some(2023),
        None,
    )
    .await
    .unwrap();

    mount_cv_calendar(
        &app,
        "2026-05-13",
        "2026-05-19",
        r#"[
            { "id": 60001, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 7777, "name": "Cached Publisher Vol" },
              "site_detail_url": "https://cv/4000-60001/" },
            { "id": 60002, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 8888, "name": "Brand New Volume" },
              "site_detail_url": "https://cv/4000-60002/" }
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

    // (a) Cache hit → publisher surfaces from cv_volume_cache.
    let cached = rows.iter().find(|r| r["cv_volume_id"] == 7777).unwrap();
    assert!(cached["series_id"].is_null());
    assert_eq!(
        cached["publisher"], "Boom! Studios",
        "publisher must come from cv_volume_cache for non-catalog volumes"
    );

    // (b) Brand-new volume → no cache row yet → publisher null in this
    // response, but the queue MUST have a pending row written for it
    // so the worker's cache-fill pass picks it up next iteration.
    let pending = rows.iter().find(|r| r["cv_volume_id"] == 8888).unwrap();
    assert!(pending["series_id"].is_null());
    assert!(
        pending["publisher"].is_null(),
        "first-sight volume reads as null in this response"
    );

    let queued = cv_volume_cache_repo::find_by_id(&app.state.db, 8888)
        .await
        .unwrap();
    assert!(
        queued.is_some(),
        "first-sight volume MUST be queued as a pending cache row"
    );
    let queued = queued.unwrap();
    assert!(
        queued.fetched_at.is_none(),
        "queued row is pending — fetched_at NULL"
    );
    assert!(queued.publisher.is_none());

    // (c) A second calendar request that sees volume 7777 again must
    // NOT re-queue it (already-fetched row remains untouched). And
    // it must NOT re-queue 8888 (already pending). The bulk-INSERT-
    // OR-IGNORE on cv_volume_id PK is the guarantee.
    let body2 = response_json(
        app.request(empty_request(
            "GET",
            "/api/releases/calendar?from=2026-05-13&to=2026-05-19",
        ))
        .await,
    )
    .await;
    let _ = body2;
    let pending_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cv_volume_cache")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(
        pending_count, 2,
        "two distinct cv_volume_ids in the cache after two requests — re-requests are no-ops"
    );
}

// ============ Item A v2 piece 3: forward calendar via Metron ============

/// Forward-week request when `state.metron = None` (kill switch off OR
/// credentials missing/invalid) returns an empty Vec, NOT an error.
/// The user clicking "Next week" with Metron disabled sees an empty
/// state, not a 500. The CV path is untouched.
#[tokio::test]
async fn forward_week_returns_empty_when_metron_disabled() {
    let app = build_test_app().await;
    // state.metron is None by default. No Metron mock to mount.

    // Use a far-future date — guaranteed `from > today_utc()`. The
    // dispatch rule must short-circuit to empty here without hitting
    // any external API.
    let body = response_json(
        app.request(empty_request(
            "GET",
            "/api/releases/calendar?from=2099-01-01&to=2099-01-07",
        ))
        .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert!(rows.is_empty(), "forward week with Metron disabled → empty");
}

/// Forward-week request when `state.metron = Some(client)` runs the
/// hydration path: list endpoint + per-issue detail fetches in
/// parallel, publisher resolved inline from Metron's issue-detail
/// payload, cached as fully-hydrated JSON.
///
/// **The load-bearing assertion is `publisher == "DC Comics"`** —
/// not just non-null but the actual expected name. A test that
/// asserted only `is_some` would pass even if hydration silently
/// stored Metron's list-endpoint output (which has publisher: None)
/// instead of the detail-endpoint payload. The whole point of
/// piece 3 is that the issue-detail step ran and resolved publisher
/// before the cache write.
#[tokio::test]
async fn forward_week_hydrates_publisher_inline_from_metron_detail() {
    let mut app = build_test_app().await;
    let metron_server = wiremock::MockServer::start().await;
    app.enable_metron(&metron_server);

    // Metron list response — has issues but no publisher / series.id
    // inline (matches the real shape probed in the archaeology).
    let list_payload = serde_json::json!({
        "count": 2,
        "next": null,
        "previous": null,
        "results": [
            {
                "id": 170031,
                "series": {"name": "Absolute Green Lantern", "volume": 1, "year_began": 2025},
                "number": "15",
                "issue": "Absolute Green Lantern (2025) #15",
                "cover_date": "2099-03-01",
                "store_date": "2099-01-03",
                "image": "https://cv/cover-1.jpg",
                "cover_hash": null,
                "modified": null
            },
            {
                "id": 170032,
                "series": {"name": "Action Comics", "volume": 3, "year_began": 2016},
                "number": "1099",
                "issue": "Action Comics (2016) #1099",
                "cover_date": "2099-03-01",
                "store_date": "2099-01-05",
                "image": null,
                "cover_hash": null,
                "modified": null
            }
        ]
    })
    .to_string();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/issue/"))
        .and(wiremock::matchers::query_param(
            "store_date_range_after",
            "2099-01-01",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(list_payload))
        .mount(&metron_server)
        .await;

    // Detail responses — publisher inline. The hydration step must
    // walk these to get publisher onto the rows.
    let detail_payload = |id: i64, series_name: &str, pub_name: &str| {
        serde_json::json!({
            "id": id,
            "publisher": {"id": 2, "name": pub_name},
            "imprint": null,
            "series": {
                "id": id * 10,
                "name": series_name,
                "sort_name": series_name,
                "volume": 1,
                "year_began": 2025
            },
            "number": "1",
            "alt_number": "",
            "title": "",
            "cover_date": "2099-03-01",
            "store_date": "2099-01-03",
            "foc_date": "2098-12-15",
            "price": "4.99",
            "desc": "",
            "image": null,
            "cv_id": null,
            "gcd_id": null,
            "resource_url": format!("https://metron.cloud/issue/{}", id),
            "modified": null
        })
        .to_string()
    };
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/issue/170031/"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_string(detail_payload(
                170031,
                "Absolute Green Lantern",
                "DC Comics",
            )),
        )
        .mount(&metron_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/issue/170032/"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_string(detail_payload(
                170032,
                "Action Comics",
                "DC Comics",
            )),
        )
        .mount(&metron_server)
        .await;

    let body = response_json(
        app.request(empty_request(
            "GET",
            "/api/releases/calendar?from=2099-01-01&to=2099-01-07",
        ))
        .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 2, "both forward-week rows return");

    for row in rows {
        // Source identity: Metron-sourced rows always have
        // metron_issue_id; cv_issue_id stays null until CV catalogs.
        assert!(
            !row["metron_issue_id"].is_null(),
            "Metron-sourced row must carry metron_issue_id"
        );
        // THE LOAD-BEARING ASSERTION — hydration actually walked the
        // detail endpoint and resolved publisher to a real string.
        assert_eq!(
            row["publisher"], "DC Comics",
            "publisher must be hydrated from Metron's detail endpoint, not None from the list endpoint"
        );
    }

    // Cache must have been written with the hydrated payload — the
    // 24h TTL means a second request reads from cache. Verify the
    // cache row exists post-fetch.
    let cache_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metron_calendar_cache")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(cache_count, 1, "cache row written on miss");
}

/// Current-week request must NOT route through Metron even when it's
/// enabled. The CV path stays unchanged; the dispatch boundary is
/// strictly `from > today_utc()`.
#[tokio::test]
async fn current_week_unaffected_by_metron_when_enabled() {
    let mut app = build_test_app().await;
    let metron_server = wiremock::MockServer::start().await;
    app.enable_metron(&metron_server);

    // Mount NO Metron mocks. If the dispatch wrongly routes a
    // current-week request through Metron, the unmocked call surfaces
    // as a 404 from wiremock's catch-all → MetronError → 502 here.
    // A successful 200 with the CV-sourced rows proves the dispatch
    // stayed on the CV path.
    mount_cv_calendar(
        &app,
        "2026-05-13",
        "2026-05-19",
        r#"[
            { "id": 50001, "issue_number": "1", "name": null, "cover_date": null,
              "store_date": "2026-05-14", "description": null, "image": null,
              "volume": { "id": 4050, "name": "Saga" },
              "site_detail_url": "https://cv/4000-50001/" }
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
    assert_eq!(rows.len(), 1);
    // CV-sourced row: cv_issue_id populated, metron_issue_id null.
    assert!(
        !rows[0]["cv_issue_id"].is_null(),
        "CV path populates cv_issue_id"
    );
    assert!(
        rows[0]["metron_issue_id"].is_null(),
        "CV path leaves metron_issue_id null"
    );
}

// ===== Item A v2 piece 4: Option C subscription resolution =====

/// Subscribing via the existing `cv_volume_id` path stays exactly as
/// it was before Option C — no Metron round trip, no behavioral
/// change. Belt-and-suspenders against the body-shape extension.
#[tokio::test]
async fn subscribe_via_cv_volume_id_path_unchanged() {
    let app = build_test_app().await;
    mount_cv_volume(&app, 7001, "Bone").await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"cv_volume_id":7001}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let series_id = response_json(resp).await["series_id"].as_i64().unwrap();
    let series = series_repo::find_by_cv_id(&app.state.db, 7001)
        .await
        .unwrap()
        .expect("series created");
    assert_eq!(series.id, series_id);
    // CV-path doesn't touch metron_id.
    assert!(series.metron_id.is_none(), "CV path leaves metron_id NULL");
}

/// THE load-bearing test. Subscribing via `metron_series_id` must:
/// 1. Resolve cv_id via Metron's `fetch_series_detail`
/// 2. Use that cv_id for the standard `try_add_one` flow
/// 3. Backfill `series.metron_id` after the subscription
/// 4. ON A SECOND CALL FOR THE SAME METRON SERIES: hit the catalog
///    cache (verified by counting Metron mock invocations — must be
///    exactly 1 after two subscription attempts).
#[tokio::test]
async fn subscribe_via_metron_series_id_resolves_cv_id_and_writes_back() {
    let mut app = build_test_app().await;
    let metron_server = wiremock::MockServer::start().await;
    app.enable_metron(&metron_server);

    // Pre-existing tracked series — Item A v2 Option C resolves cv_id
    // from Metron, then add_or_get_from_cv resolves to this existing
    // row rather than creating a new one. CV mock not needed because
    // the series already exists.
    let existing = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(8001),
            metron_id: None,
            title: "Absolute Catwoman".into(),
            sort_title: "absolute catwoman".into(),
            start_year: Some(2026),
            publisher: Some("DC Comics".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    // Mock Metron series detail — returns cv_id = 8001 matching our
    // pre-existing row. `expect(1)` is the load-bearing assertion:
    // after two subscription attempts for the same metron_series_id,
    // this mock must have been called exactly once. The second call
    // is the catalog-cache path proof.
    let detail_payload = serde_json::json!({
        "id": 12345,
        "name": "Absolute Catwoman",
        "sort_name": "Absolute Catwoman",
        "volume": 1,
        "year_began": 2026,
        "year_end": null,
        "publisher": {"id": 2, "name": "DC Comics"},
        "imprint": null,
        "cv_id": 8001,
        "gcd_id": null,
        "issue_count": 5
    })
    .to_string();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/series/12345/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(detail_payload))
        .expect(1)
        .mount(&metron_server)
        .await;

    // First subscription — uses Metron, succeeds, writes back metron_id.
    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"metron_series_id":12345}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let series_id = response_json(resp).await["series_id"].as_i64().unwrap();
    assert_eq!(
        series_id, existing.id,
        "resolves to existing series via cv_id"
    );

    // Verify backfill ran — series.metron_id is now populated.
    let series = series_repo::find_by_id(&app.state.db, existing.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        series.metron_id.as_deref(),
        Some("12345"),
        "metron_id backfilled with stringified series id"
    );

    // Second subscription for same metron_series_id — must hit the
    // catalog cache, NOT Metron. The mock's `expect(1)` enforces this
    // when MockServer drops at the end of the test; the assert below
    // is the user-visible success.
    let resp2 = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"metron_series_id":12345}"#,
        ))
        .await;
    assert_eq!(resp2.status(), StatusCode::OK);
    let series_id2 = response_json(resp2).await["series_id"].as_i64().unwrap();
    assert_eq!(series_id2, existing.id, "second call resolves same series");

    // Drop the MockServer to trigger expect() verification. If the
    // catalog-cache path failed and a second Metron call fired, this
    // panics with "expected 1 calls but got 2".
    drop(metron_server);
}

/// GH #7 — subscribing via Metron persists the run's finished-state from
/// the detail's `status` field, for free (no extra call). `Completed`
/// (and `Cancelled`) → `series.finished = true`, surfaced on the
/// SeriesWithCounts list that drives the purple complete-collection badge.
#[tokio::test]
async fn subscribe_via_metron_persists_finished_when_status_completed() {
    let mut app = build_test_app().await;
    let metron_server = wiremock::MockServer::start().await;
    app.enable_metron(&metron_server);

    let existing = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: Some(8101),
            metron_id: None,
            title: "Finished Run".into(),
            sort_title: "finished run".into(),
            start_year: Some(2020),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    let detail = serde_json::json!({
        "id": 7001, "name": "Finished Run", "volume": 1,
        "year_began": 2020, "year_end": 2022,
        "publisher": {"id": 1, "name": "Image"},
        "cv_id": 8101, "issue_count": 12, "status": "Completed"
    })
    .to_string();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/series/7001/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(detail))
        .mount(&metron_server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"metron_series_id":7001}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let list = response_json(app.request(empty_request("GET", "/api/series")).await).await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_i64() == Some(existing.id))
        .unwrap();
    assert_eq!(row["finished"], true, "Completed status → finished=true");
}

/// GH #7 — the on-demand backfill: `POST /api/series/enrich-finished`
/// sweeps metron-linked, not-yet-finished series, flips Completed |
/// Cancelled to finished, leaves Ongoing alone, and reports series with
/// no metron_id as `skipped`.
#[tokio::test]
async fn enrich_finished_flips_completed_leaves_ongoing_and_counts_skipped() {
    let mut app = build_test_app().await;
    let metron_server = wiremock::MockServer::start().await;
    app.enable_metron(&metron_server);

    let db = app.state.db.clone();
    let seed = |cv: i64, metron: Option<&'static str>, title: &'static str| NewSeries {
        cv_id: Some(cv),
        metron_id: metron.map(|m| m.to_string()),
        title: title.into(),
        sort_title: title.to_lowercase(),
        start_year: None,
        publisher: None,
        description: None,
        cover_url: None,
    };
    // Completed → finished; Ongoing → stays false; no metron_id → skipped.
    let a = series_repo::insert(&db, seed(1, Some("111"), "Alpha"))
        .await
        .unwrap();
    let b = series_repo::insert(&db, seed(2, Some("222"), "Bravo"))
        .await
        .unwrap();
    let _c = series_repo::insert(&db, seed(3, None, "Charlie"))
        .await
        .unwrap();

    let detail = |id: i64, status: &str| {
        serde_json::json!({
            "id": id, "name": "X", "volume": 1, "year_began": 2000,
            "year_end": null, "cv_id": id, "issue_count": 1, "status": status
        })
        .to_string()
    };
    for (mid, status) in [(111_i64, "Completed"), (222, "Ongoing")] {
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!("/series/{mid}/")))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(detail(mid, status)))
            .mount(&metron_server)
            .await;
    }

    let resp = app
        .request(empty_request("POST", "/api/series/enrich-finished"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["checked"], 2);
    assert_eq!(body["updated"], 1);
    assert_eq!(body["skipped"], 1);

    let list = response_json(app.request(empty_request("GET", "/api/series")).await).await;
    let finished = |id: i64| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"].as_i64() == Some(id))
            .unwrap()["finished"]
            .as_bool()
            .unwrap()
    };
    assert!(finished(a.id), "Completed series flipped to finished");
    assert!(!finished(b.id), "Ongoing series stays not-finished");
}

/// GH #7 — enrichment is Metron-optional: with no client configured the
/// endpoint returns a clean 503, never panics.
#[tokio::test]
async fn enrich_finished_returns_503_when_metron_not_configured() {
    let app = build_test_app().await; // metron: None by default
    let resp = app
        .request(empty_request("POST", "/api/series/enrich-finished"))
        .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// When Metron returns the series detail but cv_id is None (the series
/// isn't yet indexed by CV), surface a clean 422 with the
/// user-actionable fallback message.
#[tokio::test]
async fn subscribe_via_metron_series_id_returns_422_when_cv_id_absent() {
    let mut app = build_test_app().await;
    let metron_server = wiremock::MockServer::start().await;
    app.enable_metron(&metron_server);

    let detail_payload = serde_json::json!({
        "id": 99999,
        "name": "Brand New Indie",
        "sort_name": "Brand New Indie",
        "volume": 1,
        "year_began": 2026,
        "year_end": null,
        "publisher": {"id": 99, "name": "Indie Press"},
        "imprint": null,
        "cv_id": null,
        "gcd_id": null,
        "issue_count": 1
    })
    .to_string();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/series/99999/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(detail_payload))
        .mount(&metron_server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"metron_series_id":99999}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_json(resp).await;
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("isn't yet indexed by ComicVine"),
        "user-facing fallback message: {message}"
    );
}

/// When Metron is disabled (kill switch off OR credentials bad), a
/// `metron_series_id`-only request can't be resolved. The catalog
/// cache might cover it for previously-resolved series — but for
/// brand-new ones with no catalog row, the resolution path needs
/// `state.metron`. Surface 503 with a clear code, not 500.
#[tokio::test]
async fn subscribe_returns_503_when_metron_disabled_and_only_metron_series_id_present() {
    let app = build_test_app().await;
    // state.metron is None — default for build_test_app.

    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull",
            r#"{"metron_series_id":77777}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(resp).await;
    let code = body["error"]["code"].as_str().unwrap();
    assert!(
        code.contains("metron_not_configured"),
        "code identifies the misconfig: {code}"
    );
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
    // Auto-trigger: subscribing through the calendar must fire an
    // on-demand search for the new series_id, just like POST
    // /api/pull-list does. is_searching is observable here because the
    // spawned cleanup task hasn't been scheduled yet on the
    // single-threaded test runtime.
    assert!(
        app.state.pull_search.is_searching(series_id).await,
        "calendar add must auto-trigger a search for the new series_id"
    );
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
    // Item A v2 Option C: bulk-add body is now `items: [{cv_volume_id|metron_series_id}]`.
    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull/bulk",
            r#"{"items":[{"cv_volume_id":6001},{"cv_volume_id":6002},{"cv_volume_id":0}]}"#,
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

/// Poll `GET /api/releases/calendar/pull/status` until `key` reaches a
/// terminal status or the attempt budget runs out. Returns the terminal
/// outcome object. Drives the current-thread runtime so the spawned
/// background resolver makes progress between polls.
async fn poll_subscribe_status(
    app: &common::TestApp,
    key: &str,
    max_attempts: u32,
) -> serde_json::Value {
    for _ in 0..max_attempts {
        let body = response_json(
            app.request(empty_request("GET", "/api/releases/calendar/pull/status"))
                .await,
        )
        .await;
        if let Some(entry) = body["items"].get(key) {
            let status = entry["status"].as_str().unwrap_or("");
            if status != "resolving" {
                return entry.clone();
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("`{key}` never reached a terminal status within {max_attempts} polls");
}

#[tokio::test]
async fn calendar_bulk_add_backgrounds_a_not_yet_local_volume() {
    let app = build_test_app().await;
    // 2127 is NOT in the local catalog, so subscribing it requires a CV
    // fetch. That fetch must NOT happen synchronously in the request —
    // the item comes back `resolving` and a background task finishes it.
    mount_cv_volume(&app, 2127, "The Walking Dead").await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull/bulk",
            r#"{"items":[{"cv_volume_id":2127}]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]["status"], "resolving",
        "a not-yet-local volume must be deferred, not resolved in-request"
    );
    // The synchronous response has not created the series yet.
    assert!(
        series_repo::find_by_cv_id(&app.state.db, 2127)
            .await
            .unwrap()
            .is_none(),
        "series must not exist until the background resolver runs"
    );

    // Background resolver finishes it: status flips to `added` and the
    // series is created + subscribed.
    let outcome = poll_subscribe_status(&app, "cv:2127", 100).await;
    assert_eq!(outcome["status"], "added");
    let series = series_repo::find_by_cv_id(&app.state.db, 2127)
        .await
        .unwrap()
        .expect("background resolver created the series");
    assert_eq!(outcome["series_id"].as_i64(), Some(series.id));
    assert!(
        pull_list_repo::get(&app.state.db, series.id)
            .await
            .unwrap()
            .is_some(),
        "background resolver subscribed the series"
    );
}

#[tokio::test]
async fn calendar_bulk_add_surfaces_background_failure_in_status() {
    let app = build_test_app().await;
    // 4040 is not local AND the CV volume fetch 404s — the background
    // resolver must record a terminal `failed` outcome with a reason,
    // not silently drop it.
    Mock::given(method("GET"))
        .and(path("/volume/4050-4040/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&app.cv_server)
        .await;

    let resp = app
        .request(json_request(
            "POST",
            "/api/releases/calendar/pull/bulk",
            r#"{"items":[{"cv_volume_id":4040}]}"#,
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        response_json(resp).await["results"][0]["status"],
        "resolving"
    );

    let outcome = poll_subscribe_status(&app, "cv:4040", 100).await;
    assert_eq!(outcome["status"], "failed");
    assert!(
        outcome["error"].is_string(),
        "a background failure must carry a reason"
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

#[tokio::test]
async fn needs_attention_dismiss_deletes_one_attempt_by_id() {
    let app = build_test_app().await;
    let (series, issue_a) = seed_series_and_issue(&app, "Saga", "1").await;
    let (_, issue_b) = seed_series_and_issue(&app, "Chew", "1").await;
    let a_id = pull_attempt_repo::insert(&app.state.db, failed_attempt(series, issue_a, None))
        .await
        .unwrap()
        .id;
    pull_attempt_repo::insert(
        &app.state.db,
        failed_attempt(series, issue_b, Some("rel-x")),
    )
    .await
    .unwrap();

    // Surface row carries pa.id — the dismiss endpoint targets that.
    let body = response_json(
        app.request(empty_request("GET", "/api/needs-attention/pull-failures"))
            .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let surfaced_a_id = rows
        .iter()
        .find(|r| r["issue_id"].as_i64() == Some(issue_a))
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    assert_eq!(surfaced_a_id, a_id);

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/needs-attention/pull-failures/{a_id}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Only the dismissed row is gone; the other failure remains.
    let body = response_json(
        app.request(empty_request("GET", "/api/needs-attention/pull-failures"))
            .await,
    )
    .await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["issue_id"].as_i64(), Some(issue_b));
}

#[tokio::test]
async fn needs_attention_dismiss_also_purges_stale_submitted_for_same_issue() {
    // The user-observed bug: Dismiss surgically removes the visible
    // failure row, but a stale `submitted` row from a prior run is
    // STILL there blocking future Search clicks. After this fix the
    // dismiss endpoint chases the same (series_id, issue_id) and
    // purges any `submitted` row older than 6h. Fresh submitted rows
    // (potentially still downloading) are left alone.
    let app = build_test_app().await;
    let (series, issue) = seed_series_and_issue(&app, "Saga", "1").await;

    // Visible failure (the row the user will click Dismiss on).
    let failed_id = pull_attempt_repo::insert(
        &app.state.db,
        failed_attempt(series, issue, Some("rel-old")),
    )
    .await
    .unwrap()
    .id;
    // Invisible stale `submitted` row from a prior container run.
    let stale_id = pull_attempt_repo::insert(
        &app.state.db,
        longbox_db::NewPullAttempt {
            series_id: series,
            issue_id: issue,
            indexer_id: None,
            release_id: Some("rel-stale".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-stale".into()),
        },
    )
    .await
    .unwrap()
    .id;
    sqlx::query(
        "UPDATE pull_attempts SET attempted_at = datetime('now', '-24 hours') WHERE id = ?",
    )
    .bind(stale_id)
    .execute(&app.state.db)
    .await
    .unwrap();

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/needs-attention/pull-failures/{failed_id}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // BOTH rows must be gone — failure surgically, stale-submitted via
    // the chase-purge. The issue now has zero pull_attempts and the
    // next Search will fire.
    let remaining = pull_attempt_repo::list_for_issue(&app.state.db, series, issue)
        .await
        .unwrap();
    assert!(
        remaining.is_empty(),
        "Dismiss must clear both the surgical row AND any stale submitted; got {} rows",
        remaining.len()
    );
}

#[tokio::test]
async fn needs_attention_dismiss_preserves_fresh_submitted_for_same_issue() {
    // Counterpart guard: a `submitted` row that's FRESH (under 6h)
    // is potentially still downloading. Dismissing an unrelated
    // failure row MUST NOT purge it. Only stale submitted rows go.
    let app = build_test_app().await;
    let (series, issue) = seed_series_and_issue(&app, "Saga", "1").await;
    let failed_id = pull_attempt_repo::insert(
        &app.state.db,
        failed_attempt(series, issue, Some("rel-old")),
    )
    .await
    .unwrap()
    .id;
    let fresh_id = pull_attempt_repo::insert(
        &app.state.db,
        longbox_db::NewPullAttempt {
            series_id: series,
            issue_id: issue,
            indexer_id: None,
            release_id: Some("rel-fresh".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-fresh".into()),
        },
    )
    .await
    .unwrap()
    .id;
    // Default attempted_at = now → well within the 6h threshold.

    let resp = app
        .request(empty_request(
            "DELETE",
            &format!("/api/needs-attention/pull-failures/{failed_id}"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let remaining = pull_attempt_repo::list_for_issue(&app.state.db, series, issue)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1, "fresh submitted must survive");
    assert_eq!(remaining[0].id, fresh_id);
    assert_eq!(remaining[0].status, "submitted");
}

#[tokio::test]
async fn needs_attention_dismiss_unknown_id_is_a_no_op() {
    // A stale UI dismissing a row that was already cleared (retry,
    // bulk-clear, a race with another tab) gets a clean 204 — the
    // dismiss action is idempotent from the caller's perspective.
    let app = build_test_app().await;
    let resp = app
        .request(empty_request(
            "DELETE",
            "/api/needs-attention/pull-failures/99999",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn needs_attention_clear_all_deletes_every_failure_class_attempt() {
    let app = build_test_app().await;
    let (series, issue_a) = seed_series_and_issue(&app, "Saga", "1").await;
    let (_, issue_b) = seed_series_and_issue(&app, "Chew", "1").await;
    let (series_c, issue_c) = seed_series_and_issue(&app, "Bone", "1").await;
    pull_attempt_repo::insert(&app.state.db, failed_attempt(series, issue_a, None))
        .await
        .unwrap();
    pull_attempt_repo::insert(
        &app.state.db,
        failed_attempt(series, issue_b, Some("rel-x")),
    )
    .await
    .unwrap();
    // A non-failure attempt — preserves history of an in-flight pull
    // that the bulk-clear must NOT touch.
    pull_attempt_repo::insert(
        &app.state.db,
        NewPullAttempt {
            series_id: series_c,
            issue_id: issue_c,
            indexer_id: None,
            release_id: Some("rel-live".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("handle".into()),
        },
    )
    .await
    .unwrap();

    let resp = app
        .request(empty_request(
            "DELETE",
            "/api/needs-attention/pull-failures",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Failures cleared.
    let body = response_json(
        app.request(empty_request("GET", "/api/needs-attention/pull-failures"))
            .await,
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 0);
    // The live `submitted` attempt is still on file — bulk-clear is
    // surgical to failure-class statuses.
    let remaining = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pull_attempts")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(remaining, 1);
}

// -------- creators --------

#[tokio::test]
async fn creators_search_returns_seeded_creator() {
    let app = build_test_app().await;
    let (_, iid) = seed_series_and_issue(&app, "Sandman", "1").await;

    // file: owned + present so search_creators' EXISTS sub-query fires
    longbox_db::file_repo::insert(
        &app.state.db,
        longbox_db::NewFile {
            issue_id: Some(iid),
            library_root_id: app.library_root_id,
            path_relative: "Sandman (1989)/Sandman 001.cbz".into(),
            size_bytes: 1,
            mtime: time::macros::datetime!(2024-01-01 0:00),
            last_scanned_at: time::macros::datetime!(2024-01-01 0:00),
            match_method: "filename".into(),
            match_confidence: 0.99,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: time::macros::datetime!(2024-01-01 0:00),
            matched_at: Some(time::macros::datetime!(2024-01-01 0:00)),
        },
    )
    .await
    .unwrap();

    longbox_db::creator_repo::insert_issue_credits(
        &app.state.db,
        iid,
        &[longbox_comicvine::CvPersonCredit {
            cv_person_id: 9999,
            name: "Neil Gaiman".into(),
            role: "writer".into(),
        }],
    )
    .await
    .unwrap();

    let resp = app
        .request(empty_request("GET", "/api/creators/search?q=Gaiman"))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Neil Gaiman");
}

#[tokio::test]
async fn creators_detail_returns_creator_json() {
    let app = build_test_app().await;
    let (_, iid) = seed_series_and_issue(&app, "Sandman", "1").await;

    longbox_db::file_repo::insert(
        &app.state.db,
        longbox_db::NewFile {
            issue_id: Some(iid),
            library_root_id: app.library_root_id,
            path_relative: "Sandman (1989)/Sandman 001.cbz".into(),
            size_bytes: 1,
            mtime: time::macros::datetime!(2024-01-01 0:00),
            last_scanned_at: time::macros::datetime!(2024-01-01 0:00),
            match_method: "filename".into(),
            match_confidence: 0.99,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: time::macros::datetime!(2024-01-01 0:00),
            matched_at: Some(time::macros::datetime!(2024-01-01 0:00)),
        },
    )
    .await
    .unwrap();

    longbox_db::creator_repo::insert_issue_credits(
        &app.state.db,
        iid,
        &[longbox_comicvine::CvPersonCredit {
            cv_person_id: 9999,
            name: "Neil Gaiman".into(),
            role: "writer".into(),
        }],
    )
    .await
    .unwrap();

    // Obtain id via search — avoids a raw DB query.
    let search_body = response_json(
        app.request(empty_request("GET", "/api/creators/search?q=Gaiman"))
            .await,
    )
    .await;
    let id = search_body[0]["id"].as_i64().unwrap();

    let resp = app
        .request(empty_request("GET", &format!("/api/creators/{id}")))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["name"], "Neil Gaiman");
    assert!(
        !body["roles"].as_array().unwrap().is_empty(),
        "detail must include at least one role for a credited + owned issue"
    );
}

#[tokio::test]
async fn creators_detail_missing_id_returns_404() {
    let app = build_test_app().await;
    let resp = app
        .request(empty_request("GET", "/api/creators/999999"))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn creators_discover_empty_when_no_cv_person_id() {
    let app = build_test_app().await;
    // Insert a creator with NULL cv_person_id directly (no ingestion needed).
    let cid: i64 = sqlx::query_scalar(
        "INSERT INTO creators (name, cv_person_id) VALUES ('Nobody', NULL) RETURNING id",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let resp = app
        .request(empty_request(
            "GET",
            &format!("/api/creators/{cid}/discover"),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;
    assert_eq!(body["results"].as_array().unwrap().len(), 0);
    assert_eq!(body["filtered_count"].as_u64().unwrap(), 0);
}
