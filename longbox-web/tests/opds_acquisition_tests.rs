//! OPDS acquisition feed (issue list per series) and search (commit 3).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::{build_test_app, response_text, TestApp};
use longbox_db::{file_repo, issue_repo, series_repo, NewFile, NewIssue, NewSeries};

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

fn now_pdt() -> time::PrimitiveDateTime {
    let n = time::OffsetDateTime::now_utc();
    time::PrimitiveDateTime::new(n.date(), n.time())
}

async fn seed_series(db: &longbox_db::Pool, title: &str) -> i64 {
    series_repo::insert(
        db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.to_owned(),
            sort_title: title.to_lowercase(),
            start_year: Some(2021),
            publisher: Some("Image".to_owned()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

async fn seed_issue(
    db: &longbox_db::Pool,
    series_id: i64,
    number: &str,
    cover_date: Option<&str>,
    summary: Option<&str>,
    cover_url: Option<&str>,
) -> i64 {
    issue_repo::insert(
        db,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.to_owned(),
            title: None,
            cover_date: cover_date.map(str::to_owned),
            summary: summary.map(str::to_owned),
            cover_url: cover_url.map(str::to_owned),
        },
    )
    .await
    .unwrap()
    .id
}

async fn seed_file(
    db: &longbox_db::Pool,
    root_id: i64,
    issue_id: i64,
    filename: &str,
    status: &str,
    is_present: bool,
) {
    let now = now_pdt();
    file_repo::insert(
        db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: root_id,
            path_relative: filename.to_owned(),
            size_bytes: 1234,
            mtime: now,
            last_scanned_at: now,
            match_method: "test".to_owned(),
            match_confidence: 1.0,
            status: status.to_owned(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present,
            last_seen_at: now,
            matched_at: Some(now),
        },
    )
    .await
    .unwrap();
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

async fn body_of(app: &TestApp, uri: &str, expect_type: &str) -> String {
    let resp = app.request(get(uri)).await;
    assert_eq!(resp.status(), StatusCode::OK, "non-200 for {uri}");
    assert_eq!(content_type(&resp), expect_type, "wrong content-type for {uri}");
    response_text(resp).await
}

#[tokio::test]
async fn series_detail_lists_issues_with_conditional_acquisition_links() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let root_id = app.library_root_id;
    let series = seed_series(&app.state.db, "Saga").await;

    // #1: owned present cbz → has cover + acquisition link.
    let i1 = seed_issue(
        &app.state.db,
        series,
        "1",
        Some("2024-01-15"),
        Some("The beginning"),
        Some("https://cdn/c1.jpg"),
    )
    .await;
    seed_file(&app.state.db, root_id, i1, "Saga/Saga 001.cbz", "owned", true).await;

    // #2: no file → no acquisition link, but has a cover.
    seed_issue(
        &app.state.db,
        series,
        "2",
        Some("2024-02-15"),
        None,
        Some("https://cdn/c2.jpg"),
    )
    .await;

    let xml = body_of(&app, &format!("/opds/v1/series/{series}"), ACQ_TYPE).await;
    assert_eq!(xml.matches("<entry>").count(), 2);
    assert!(xml.contains("<opensearch:totalResults>2</opensearch:totalResults>"));

    // #1 carries image + acquisition (cbz) links, dc:issued, summary.
    assert!(xml.contains(&format!(
        r#"href="http://opds.test/opds/v1/issues/{i1}/download" type="application/x-cbz""#
    )));
    assert!(xml.contains(&format!(
        r#"rel="http://opds-spec.org/image" href="http://opds.test/opds/v1/covers/{i1}""#
    )));
    assert!(xml.contains("<dc:issued>2024-01-15</dc:issued>"));
    assert!(xml.contains("<summary>The beginning</summary>"));

    // Exactly one acquisition link in the whole feed (only #1 has a file).
    assert_eq!(
        xml.matches(r#"rel="http://opds-spec.org/acquisition""#).count(),
        1
    );
}

#[tokio::test]
async fn series_detail_404_for_unknown_series() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get("/opds/v1/series/9999")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn acquisition_mime_follows_extension() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let root_id = app.library_root_id;
    let series = seed_series(&app.state.db, "Rar").await;
    let issue = seed_issue(&app.state.db, series, "1", None, None, None).await;
    seed_file(&app.state.db, root_id, issue, "Rar/Rar 001.cbr", "owned", true).await;

    let xml = body_of(&app, &format!("/opds/v1/series/{series}"), ACQ_TYPE).await;
    assert!(xml.contains(r#"type="application/x-cbr""#));
}

#[tokio::test]
async fn issues_ordered_numerically_not_lexically() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let series = seed_series(&app.state.db, "Order").await;
    // Insert out of order and with a multi-digit number.
    seed_issue(&app.state.db, series, "10", None, None, None).await;
    seed_issue(&app.state.db, series, "2", None, None, None).await;
    seed_issue(&app.state.db, series, "1", None, None, None).await;

    let xml = body_of(&app, &format!("/opds/v1/series/{series}"), ACQ_TYPE).await;
    let p1 = xml.find("Issue #1<").unwrap();
    let p2 = xml.find("Issue #2<").unwrap();
    let p10 = xml.find("Issue #10<").unwrap();
    assert!(p1 < p2 && p2 < p10, "expected 1 < 2 < 10 ordering");
}

#[tokio::test]
async fn search_matches_series_titles() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let batman = seed_series(&app.state.db, "Batman").await;
    seed_series(&app.state.db, "Batgirl").await;
    seed_series(&app.state.db, "Superman").await;

    let xml = body_of(&app, "/opds/v1/search?q=Bat", NAV_TYPE).await;
    assert_eq!(xml.matches("<entry>").count(), 2);
    assert!(xml.contains("Batman"));
    assert!(xml.contains("Batgirl"));
    assert!(!xml.contains("Superman"));
    // Results link to each series' acquisition feed.
    assert!(xml.contains(&format!(
        r#"href="http://opds.test/opds/v1/series/{batman}" type="{ACQ_TYPE}""#
    )));
    assert!(xml.contains("<opensearch:totalResults>2</opensearch:totalResults>"));
}

#[tokio::test]
async fn search_empty_query_returns_empty_feed() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    seed_series(&app.state.db, "Batman").await;

    let xml = body_of(&app, "/opds/v1/search?q=", NAV_TYPE).await;
    assert_eq!(xml.matches("<entry>").count(), 0);
    assert!(xml.contains("<opensearch:totalResults>0</opensearch:totalResults>"));
}

#[tokio::test]
async fn search_treats_percent_as_literal() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    seed_series(&app.state.db, "50% Off").await;
    seed_series(&app.state.db, "Batman").await;

    // The '%' is escaped, so it matches the literal title — not as a
    // wildcard that would also pull in "Batman".
    let xml = body_of(&app, "/opds/v1/search?q=50%25", NAV_TYPE).await;
    assert_eq!(xml.matches("<entry>").count(), 1);
    assert!(xml.contains("50% Off"));
}
