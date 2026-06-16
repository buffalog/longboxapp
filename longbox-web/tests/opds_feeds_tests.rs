//! OPDS navigation feeds (commit 2): catalog root, all-series, publisher
//! list, per-publisher series, and pagination. Auth is configured up front;
//! the gate itself is covered in opds_auth_tests.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::{build_test_app, response_text, TestApp};
use longbox_db::{series_repo, NewSeries};

const USERNAME: &str = "reader";
const PASSWORD: &str = "hunter2";
const NAV_TYPE: &str = "application/atom+xml;profile=opds-catalog;kind=navigation";
const ACQ_TYPE: &str = "application/atom+xml;profile=opds-catalog;kind=acquisition";

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

async fn seed_series(db: &longbox_db::Pool, title: &str, publisher: Option<&str>) -> i64 {
    series_repo::insert(
        db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.to_owned(),
            sort_title: title.to_lowercase(),
            start_year: Some(2020),
            publisher: publisher.map(str::to_owned),
            description: None,
            cover_url: None,
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

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, basic())
        .body(Body::empty())
        .unwrap()
}

fn content_type(resp: &axum::response::Response) -> String {
    resp.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn body_of(app: &TestApp, uri: &str) -> String {
    let resp = app.request(get(uri)).await;
    assert_eq!(resp.status(), StatusCode::OK, "non-200 for {uri}");
    assert_eq!(content_type(&resp), NAV_TYPE, "wrong content-type for {uri}");
    response_text(resp).await
}

#[tokio::test]
async fn root_lists_series_and_publishers() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;

    let xml = body_of(&app, "/opds/v1").await;
    assert!(xml.contains("<title>All Series</title>"));
    assert!(xml.contains("<title>Publishers</title>"));
    // Absolute hrefs from the configured base URL (http://opds.test).
    assert!(xml.contains(r#"href="http://opds.test/opds/v1/series""#));
    assert!(xml.contains(r#"href="http://opds.test/opds/v1/publishers""#));
}

#[tokio::test]
async fn series_feed_lists_all_series_with_acquisition_links() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let id_a = seed_series(&app.state.db, "Alpha", Some("Marvel Comics")).await;
    seed_series(&app.state.db, "Bravo", Some("Marvel Comics")).await;
    seed_series(&app.state.db, "Charlie", Some("DC Comics")).await;

    let xml = body_of(&app, "/opds/v1/series").await;
    assert_eq!(xml.matches("<entry>").count(), 3);
    assert!(xml.contains("Alpha (2020)"));
    assert!(xml.contains("<opensearch:totalResults>3</opensearch:totalResults>"));
    assert!(xml.contains("<opensearch:startIndex>1</opensearch:startIndex>"));
    // No pagination overflow → no next link.
    assert!(!xml.contains(r#"rel="next""#));
    // Series entries point at their acquisition feed.
    assert!(xml.contains(&format!(
        r#"href="http://opds.test/opds/v1/series/{id_a}" type="{ACQ_TYPE}""#
    )));
}

#[tokio::test]
async fn publishers_feed_groups_and_counts() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    seed_series(&app.state.db, "Alpha", Some("Marvel Comics")).await;
    seed_series(&app.state.db, "Bravo", Some("Marvel Comics")).await;
    seed_series(&app.state.db, "Charlie", Some("DC Comics")).await;
    // A publisher-less series must NOT create a phantom publisher entry.
    seed_series(&app.state.db, "Orphan", None).await;

    let xml = body_of(&app, "/opds/v1/publishers").await;
    assert_eq!(xml.matches("<entry>").count(), 2);
    assert!(xml.contains("<title>Marvel Comics</title>"));
    assert!(xml.contains("<title>DC Comics</title>"));
    assert!(xml.contains("<content type=\"text\">2 series</content>"));
    assert!(xml.contains("<content type=\"text\">1 series</content>"));
    // Publisher name is percent-encoded in the sub-feed href.
    assert!(xml.contains("/opds/v1/publishers/Marvel%20Comics/series"));
    assert!(xml.contains(&format!(r#"type="{NAV_TYPE}""#)));
}

#[tokio::test]
async fn publisher_series_filters_to_that_publisher() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    seed_series(&app.state.db, "Alpha", Some("Marvel Comics")).await;
    seed_series(&app.state.db, "Bravo", Some("Marvel Comics")).await;
    seed_series(&app.state.db, "Charlie", Some("DC Comics")).await;

    let xml = body_of(&app, "/opds/v1/publishers/Marvel%20Comics/series").await;
    assert_eq!(xml.matches("<entry>").count(), 2);
    assert!(xml.contains("Alpha (2020)"));
    assert!(xml.contains("Bravo (2020)"));
    assert!(!xml.contains("Charlie"));
    assert!(xml.contains("<opensearch:totalResults>2</opensearch:totalResults>"));
}

#[tokio::test]
async fn series_feed_paginates_at_25() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    for i in 0..30 {
        seed_series(&app.state.db, &format!("Series {i:03}"), Some("Marvel Comics")).await;
    }

    // Page 1: 25 entries, next but no previous.
    let p1 = body_of(&app, "/opds/v1/series?page=1").await;
    assert_eq!(p1.matches("<entry>").count(), 25);
    assert!(p1.contains("<opensearch:totalResults>30</opensearch:totalResults>"));
    assert!(p1.contains("<opensearch:itemsPerPage>25</opensearch:itemsPerPage>"));
    assert!(p1.contains("<opensearch:startIndex>1</opensearch:startIndex>"));
    assert!(p1.contains(r#"rel="next""#));
    assert!(p1.contains("/opds/v1/series?page=2"));
    assert!(!p1.contains(r#"rel="previous""#));

    // Page 2: remaining 5 entries, previous but no next.
    let p2 = body_of(&app, "/opds/v1/series?page=2").await;
    assert_eq!(p2.matches("<entry>").count(), 5);
    assert!(p2.contains("<opensearch:startIndex>26</opensearch:startIndex>"));
    assert!(p2.contains(r#"rel="previous""#));
    assert!(p2.contains("/opds/v1/series?page=1"));
    assert!(!p2.contains(r#"rel="next""#));
}

#[tokio::test]
async fn feed_routes_require_auth() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    // No Authorization header → 401, gate covers the new routes too.
    let resp = app
        .request(
            Request::builder()
                .method("GET")
                .uri("/opds/v1/series")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
