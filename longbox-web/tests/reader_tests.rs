//! Built-in reader endpoints: page count, page image streaming, and reading
//! progress. Exercises the real HTTP surface against a CBZ written to disk
//! under a temp library root.

mod common;

use std::io::Write;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{build_test_app, TestApp};
use longbox_db::{file_repo, issue_repo, series_repo, NewFile, NewIssue, NewSeries};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn now_pdt() -> time::PrimitiveDateTime {
    let n = time::OffsetDateTime::now_utc();
    time::PrimitiveDateTime::new(n.date(), n.time())
}

async fn seed_issue(db: &longbox_db::Pool) -> i64 {
    let series = series_repo::insert(
        db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Reader".to_owned(),
            sort_title: "reader".to_owned(),
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
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

/// Write a CBZ under the library root with the given entries, then register a
/// present owned file row for `issue_id` pointing at it.
async fn seed_cbz(app: &TestApp, issue_id: i64, rel: &str, entries: &[(&str, &[u8])]) {
    let full = app.library_path().join(rel);
    tokio::fs::create_dir_all(full.parent().unwrap())
        .await
        .unwrap();
    let file = std::fs::File::create(&full).unwrap();
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();

    let now = now_pdt();
    file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: app.library_root_id,
            path_relative: rel.to_owned(),
            size_bytes: 1,
            mtime: now,
            last_scanned_at: now,
            match_method: "test".to_owned(),
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
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(resp).await).unwrap()
}

/// A three-page CBZ with a non-image entry and bare (unpadded) page numbers,
/// so the page count and natural sort are both exercised.
fn three_pages() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("10.jpg", b"PAGE10"),
        ("2.png", b"PAGE2"),
        ("1.jpg", b"PAGE1"),
        ("ComicInfo.xml", b"<ComicInfo/>"),
    ]
}

#[tokio::test]
async fn pages_count_ignores_non_images() {
    let app = build_test_app().await;
    let issue = seed_issue(&app.state.db).await;
    seed_cbz(&app, issue, "Reader/Reader 001.cbz", &three_pages()).await;

    let resp = app
        .request(get(&format!("/api/issues/{issue}/pages/count")))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await, serde_json::json!({ "count": 3 }));
}

#[tokio::test]
async fn page_image_streams_in_natural_order_with_mime() {
    let app = build_test_app().await;
    let issue = seed_issue(&app.state.db).await;
    seed_cbz(&app, issue, "Reader/Reader 001.cbz", &three_pages()).await;

    // Natural sort: 1.jpg, 2.png, 10.jpg. Page 1 is a jpeg, page 2 a png.
    let p1 = app
        .request(get(&format!("/api/issues/{issue}/pages/1")))
        .await;
    assert_eq!(p1.status(), StatusCode::OK);
    assert_eq!(
        p1.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );
    assert_eq!(body_bytes(p1).await, b"PAGE1");

    let p2 = app
        .request(get(&format!("/api/issues/{issue}/pages/2")))
        .await;
    assert_eq!(p2.headers().get(header::CONTENT_TYPE).unwrap(), "image/png");
    assert_eq!(body_bytes(p2).await, b"PAGE2");

    let p3 = app
        .request(get(&format!("/api/issues/{issue}/pages/3")))
        .await;
    assert_eq!(body_bytes(p3).await, b"PAGE10");
}

#[tokio::test]
async fn page_out_of_range_is_404() {
    let app = build_test_app().await;
    let issue = seed_issue(&app.state.db).await;
    seed_cbz(&app, issue, "Reader/Reader 001.cbz", &three_pages()).await;

    let resp = app
        .request(get(&format!("/api/issues/{issue}/pages/4")))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn issue_detail_exposes_series_id() {
    let app = build_test_app().await;
    let issue = seed_issue(&app.state.db).await;
    let resp = app.request(get(&format!("/api/issues/{issue}"))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["id"], issue);
    assert!(json["series_id"].is_i64());
}

#[tokio::test]
async fn reading_progress_defaults_to_one_then_round_trips() {
    let app = build_test_app().await;
    let issue = seed_issue(&app.state.db).await;

    // Never opened → page 1, never 404.
    let resp = app
        .request(get(&format!("/api/issues/{issue}/reading-progress")))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await, serde_json::json!({ "last_page": 1 }));

    // PUT a position, then GET it back.
    let put = Request::builder()
        .method("PUT")
        .uri(format!("/api/issues/{issue}/reading-progress"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"last_page":9}"#))
        .unwrap();
    let put_resp = app.request(put).await;
    assert_eq!(put_resp.status(), StatusCode::OK);
    assert_eq!(body_json(put_resp).await, serde_json::json!({ "ok": true }));

    let resp = app
        .request(get(&format!("/api/issues/{issue}/reading-progress")))
        .await;
    assert_eq!(body_json(resp).await, serde_json::json!({ "last_page": 9 }));
}

#[tokio::test]
async fn issue_without_file_is_404_for_pages() {
    let app = build_test_app().await;
    let issue = seed_issue(&app.state.db).await;
    // No file row seeded → nothing to read.
    let resp = app
        .request(get(&format!("/api/issues/{issue}/pages/count")))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
