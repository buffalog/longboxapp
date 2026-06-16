//! OPDS cover proxy + local cache (commit 4).

mod common;

use std::path::Path;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::{build_test_app, TestApp};
use longbox_db::{issue_repo, series_repo, NewIssue, NewSeries};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USERNAME: &str = "reader";
const PASSWORD: &str = "hunter2";
const IMG: &[u8] = b"\xFF\xD8\xFF\xE0fake-jpeg-bytes";

async fn configure_opds(db: &longbox_db::Pool) {
    use longbox_db::settings_repo as s;
    s::set(db, s::KEY_OPDS_ENABLED, "true").await.unwrap();
    s::set(db, s::KEY_OPDS_USERNAME, USERNAME).await.unwrap();
    s::set(
        db,
        s::KEY_OPDS_PASSWORD_HASH,
        &longbox_opds::hash_password(PASSWORD).unwrap(),
    )
    .await
    .unwrap();
}

async fn seed_issue(db: &longbox_db::Pool, cover_url: Option<&str>) -> i64 {
    let series = series_repo::insert(
        db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Cover Test".to_owned(),
            sort_title: "cover test".to_owned(),
            start_year: Some(2020),
            publisher: Some("Image".to_owned()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    issue_repo::insert(
        db,
        NewIssue {
            series_id: series,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".to_owned(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: cover_url.map(str::to_owned),
        },
    )
    .await
    .unwrap()
    .id
}

fn basic() -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{USERNAME}:{PASSWORD}"));
    format!("Basic {encoded}")
}

fn get(uri: &str, auth: bool) -> Request<Body> {
    let mut b = Request::builder().method("GET").uri(uri);
    if auth {
        b = b.header(header::AUTHORIZATION, basic());
    }
    b.body(Body::empty()).unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn wait_for_file(path: &Path) -> bool {
    for _ in 0..100 {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Stand up a CDN mock serving IMG at /cover.jpg, returning its base URI.
async fn cdn_serving_image() -> MockServer {
    let cdn = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cover.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(IMG))
        .mount(&cdn)
        .await;
    cdn
}

#[tokio::test]
async fn fetches_from_cdn_serves_jpeg_and_caches() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let cdn = cdn_serving_image().await;
    let issue = seed_issue(&app.state.db, Some(&format!("{}/cover.jpg", cdn.uri()))).await;

    let resp = app.request(get(&format!("/opds/v1/covers/{issue}"), true)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );
    assert_eq!(body_bytes(resp).await, IMG);

    // Write-behind cache lands the bytes in covers_dir/{issue}.jpg.
    let cached = app.covers_dir.path().join(format!("{issue}.jpg"));
    assert!(wait_for_file(&cached).await, "cache file never written");
    assert_eq!(tokio::fs::read(&cached).await.unwrap(), IMG);
}

#[tokio::test]
async fn serves_from_cache_without_hitting_cdn() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    // Point cover_url at a CDN that would 500 if contacted...
    let cdn = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&cdn)
        .await;
    let issue = seed_issue(&app.state.db, Some(&format!("{}/cover.jpg", cdn.uri()))).await;

    // ...but pre-populate the cache; the handler must serve it and never
    // contact the (failing) CDN.
    let cached = app.covers_dir.path().join(format!("{issue}.jpg"));
    tokio::fs::write(&cached, b"cached-bytes").await.unwrap();

    let resp = app.request(get(&format!("/opds/v1/covers/{issue}"), true)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp).await, b"cached-bytes");
}

#[tokio::test]
async fn null_cover_url_is_404() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db, None).await;
    let resp = app.request(get(&format!("/opds/v1/covers/{issue}"), true)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_issue_is_404() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get("/opds/v1/covers/424242", true)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cdn_failure_is_502() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let cdn = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&cdn)
        .await;
    let issue = seed_issue(&app.state.db, Some(&format!("{}/cover.jpg", cdn.uri()))).await;

    let resp = app.request(get(&format!("/opds/v1/covers/{issue}"), true)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn cover_requires_auth() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db, None).await;
    let resp = app.request(get(&format!("/opds/v1/covers/{issue}"), false)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
