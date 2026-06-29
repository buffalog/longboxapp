//! End-to-end OPDS smoke test (commit 8): configure the catalog through
//! the real settings API, seed a realistic library, then walk the whole
//! catalog the way a reader would — following each advertised URL from one
//! feed to the next and confirming it actually works.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::{build_test_app, json_request, response_json, response_text, TestApp};
use longbox_db::{file_repo, issue_repo, series_repo, NewFile, NewIssue, NewSeries};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USER: &str = "reader";
const PASS: &str = "s3cretpass";
const COVER_BYTES: &[u8] = b"\xFF\xD8\xFFsmoke-cover";
const FILE_BYTES: &[u8] = b"PK\x03\x04smoke-cbz-payload";

fn now_pdt() -> time::PrimitiveDateTime {
    let n = time::OffsetDateTime::now_utc();
    time::PrimitiveDateTime::new(n.date(), n.time())
}

fn basic() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASS}"))
    )
}

/// Authenticated GET against an OPDS path (Basic auth).
async fn get(app: &TestApp, path: &str) -> axum::response::Response {
    app.request(
        Request::builder()
            .method("GET")
            .uri(path)
            .header(header::AUTHORIZATION, basic())
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// Assert 200 + expected content-type, return the body text.
async fn get_ok_text(app: &TestApp, path: &str, ctype_contains: &str) -> String {
    let resp = get(app, path).await;
    assert_eq!(resp.status(), StatusCode::OK, "GET {path} not 200");
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        ct.contains(ctype_contains),
        "GET {path} content-type {ct:?} lacks {ctype_contains:?}"
    );
    response_text(resp).await
}

#[tokio::test]
async fn full_catalog_walkthrough() {
    let app = build_test_app().await;

    // 1. A reader-facing cover CDN.
    let cdn = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cover.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(COVER_BYTES))
        .mount(&cdn)
        .await;

    // 2. Configure OPDS through the real admin API (not direct DB writes):
    //    flip the global toggle, then create a user account.
    let configured = response_json(
        app.request(json_request(
            "PUT",
            "/api/opds/settings",
            serde_json::json!({ "enabled": true }).to_string(),
        ))
        .await,
    )
    .await;
    assert_eq!(configured["enabled"], true);
    let created = app
        .request(json_request(
            "POST",
            "/api/opds/users",
            serde_json::json!({ "username": USER, "password": PASS }).to_string(),
        ))
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    // 3. Seed a realistic series → issue → present file, with a cover.
    let series_id = series_repo::insert(
        &app.state.db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Saga".to_owned(),
            sort_title: "saga".to_owned(),
            start_year: Some(2012),
            publisher: Some("Image Comics".to_owned()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".to_owned(),
            title: Some("Chapter One".to_owned()),
            cover_date: Some("2012-03-14".to_owned()),
            summary: Some("Two soldiers from opposite sides.".to_owned()),
            cover_url: Some(format!("{}/cover.jpg", cdn.uri())),
        },
    )
    .await
    .unwrap()
    .id;
    let rel = "Saga/Saga 001.cbz";
    let full = app.library_path().join(rel);
    tokio::fs::create_dir_all(full.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&full, FILE_BYTES).await.unwrap();
    let now = now_pdt();
    file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: app.library_root_id,
            path_relative: rel.to_owned(),
            size_bytes: FILE_BYTES.len() as i64,
            mtime: now,
            last_scanned_at: now,
            match_method: "smoke".to_owned(),
            match_confidence: 1.0,
            status: "owned".to_owned(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now,
            matched_at: Some(now),
        },
    )
    .await
    .unwrap();

    // 4. Root feed advertises All Series + search.
    let root = get_ok_text(&app, "/opds/v1", "kind=navigation").await;
    assert!(root.contains(r#"href="http://opds.test/opds/v1/series""#));
    assert!(root.contains(r#"rel="search""#));

    // 5. Series feed lists Saga and links to its acquisition feed.
    let series_feed = get_ok_text(&app, "/opds/v1/series", "kind=navigation").await;
    assert!(series_feed.contains("Saga (2012)"));
    let detail_path = format!("/opds/v1/series/{series_id}");
    assert!(series_feed.contains(&format!("http://opds.test{detail_path}")));

    // 6. Acquisition feed: the issue with image + download links, dc:issued,
    //    summary.
    let detail = get_ok_text(&app, &detail_path, "kind=acquisition").await;
    assert!(detail.contains("Issue #1: Chapter One"));
    assert!(detail.contains("<dc:issued>2012-03-14</dc:issued>"));
    assert!(detail.contains("Two soldiers from opposite sides."));
    let cover_path = format!("/opds/v1/covers/{issue_id}");
    let download_path = format!("/opds/v1/issues/{issue_id}/download");
    assert!(detail.contains(&format!(r#"href="http://opds.test{cover_path}""#)));
    assert!(detail.contains(&format!(
        r#"href="http://opds.test{download_path}" type="application/x-cbz""#
    )));

    // 7. The advertised cover URL fetches + caches the CDN image.
    let cover = get(&app, &cover_path).await;
    assert_eq!(cover.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(cover.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
        COVER_BYTES
    );

    // 8. The advertised download URL streams the real file.
    let download = get(&app, &download_path).await;
    assert_eq!(download.status(), StatusCode::OK);
    let cd = download
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(cd.contains(r#"filename="Saga 001.cbz""#));
    assert_eq!(
        axum::body::to_bytes(download.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
        FILE_BYTES
    );

    // 9. Search finds the series; OpenSearch descriptor is served.
    let search = get_ok_text(&app, "/opds/v1/search?q=Sag", "kind=navigation").await;
    assert!(search.contains("Saga (2012)"));
    let descriptor = get_ok_text(&app, "/opds/v1/opensearch.xml", "opensearchdescription").await;
    assert!(descriptor.contains("<ShortName>LongBox</ShortName>"));

    // 10. Unauthenticated access is refused.
    let anon = app
        .request(
            Request::builder()
                .method("GET")
                .uri("/opds/v1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        anon.headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Basic realm=\"LongBox\"")
    );
}
