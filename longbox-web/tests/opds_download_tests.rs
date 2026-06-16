//! OPDS file streaming: GET /opds/v1/issues/{id}/download (commit 5).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::{build_test_app, TestApp};
use longbox_db::{file_repo, issue_repo, series_repo, NewFile, NewIssue, NewSeries};

const USERNAME: &str = "reader";
const PASSWORD: &str = "hunter2";

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

async fn seed_issue(db: &longbox_db::Pool) -> i64 {
    let series = series_repo::insert(
        db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Dl".to_owned(),
            sort_title: "dl".to_owned(),
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

/// Insert a file row; optionally write its bytes under the library root.
async fn seed_file(
    app: &TestApp,
    issue_id: i64,
    rel: &str,
    status: &str,
    is_present: bool,
    contents: Option<&[u8]>,
) {
    if let Some(bytes) = contents {
        let full = app.library_path().join(rel);
        tokio::fs::create_dir_all(full.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&full, bytes).await.unwrap();
    }
    let now = now_pdt();
    file_repo::insert(
        &app.state.db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: app.library_root_id,
            path_relative: rel.to_owned(),
            size_bytes: contents.map(|c| c.len() as i64).unwrap_or(0),
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

fn get(uri: &str, auth: bool) -> Request<Body> {
    let mut b = Request::builder().method("GET").uri(uri);
    if auth {
        b = b.header(header::AUTHORIZATION, basic());
    }
    b.body(Body::empty()).unwrap()
}

fn header_str(resp: &axum::response::Response, name: header::HeaderName) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn streams_present_file_with_headers() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db).await;
    let contents = b"PK\x03\x04 not really a zip but bytes are bytes";
    seed_file(&app, issue, "Dl/Dl 001.cbz", "owned", true, Some(contents)).await;

    let resp = app
        .request(get(&format!("/opds/v1/issues/{issue}/download"), true))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, header::CONTENT_TYPE), "application/x-cbz");
    assert_eq!(
        header_str(&resp, header::CONTENT_LENGTH),
        contents.len().to_string()
    );
    let cd = header_str(&resp, header::CONTENT_DISPOSITION);
    assert!(cd.contains(r#"filename="Dl 001.cbz""#), "got {cd}");
    assert!(cd.contains("attachment"));
    // Path is never leaked — only the basename appears.
    assert!(!cd.contains("Dl/Dl"));

    assert_eq!(body_bytes(resp).await, contents);
}

#[tokio::test]
async fn content_type_follows_extension() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db).await;
    seed_file(&app, issue, "Dl/Dl 001.cbr", "owned", true, Some(b"rar-bytes")).await;

    let resp = app
        .request(get(&format!("/opds/v1/issues/{issue}/download"), true))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_str(&resp, header::CONTENT_TYPE), "application/x-cbr");
}

#[tokio::test]
async fn no_present_file_is_404() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db).await;
    // A row exists but is_present = false → not downloadable.
    seed_file(&app, issue, "Dl/Dl 001.cbz", "owned", false, Some(b"x")).await;

    let resp = app
        .request(get(&format!("/opds/v1/issues/{issue}/download"), true))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn present_row_but_missing_on_disk_is_404() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db).await;
    // is_present = true but no bytes written → file vanished from disk.
    seed_file(&app, issue, "Dl/Dl 001.cbz", "owned", true, None).await;

    let resp = app
        .request(get(&format!("/opds/v1/issues/{issue}/download"), true))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn absolute_path_relative_is_refused() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db).await;
    // An absolute path would let Path::join escape the library root and
    // stream /etc/hosts (which exists). Containment must 404 it; the row
    // is is_present=true and the target file genuinely exists, so a 404
    // here proves containment, not a missing file.
    seed_file(&app, issue, "/etc/hosts", "owned", true, None).await;

    let resp = app
        .request(get(&format!("/opds/v1/issues/{issue}/download"), true))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn parent_traversal_path_relative_is_refused() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db).await;
    seed_file(
        &app,
        issue,
        "../../../../../../etc/hosts",
        "owned",
        true,
        None,
    )
    .await;

    let resp = app
        .request(get(&format!("/opds/v1/issues/{issue}/download"), true))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_issue_is_404() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get("/opds/v1/issues/999/download", true)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_requires_auth() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let issue = seed_issue(&app.state.db).await;
    seed_file(&app, issue, "Dl/Dl 001.cbz", "owned", true, Some(b"x")).await;

    let resp = app
        .request(get(&format!("/opds/v1/issues/{issue}/download"), false))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
