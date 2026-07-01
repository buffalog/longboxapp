//! OPDS auth middleware (per-user accounts): 503 when the global toggle is
//! off, 401 with the Basic challenge when credentials are missing/wrong/from a
//! disabled account, passthrough for a valid enabled account. Bearer tokens
//! are no longer accepted. Feed bodies are exercised elsewhere; here we assert
//! the gate's status codes plus the last_seen side effect.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::build_test_app;
use longbox_db::{opds_users_repo, settings_repo};

const USERNAME: &str = "reader";
const PASSWORD: &str = "hunter2";

/// Enable OPDS globally and create one enabled account.
async fn configure_opds(db: &longbox_db::Pool) {
    settings_repo::set(db, settings_repo::KEY_OPDS_ENABLED, "true")
        .await
        .unwrap();
    opds_users_repo::create(
        db,
        USERNAME,
        &longbox_opds::hash_password(PASSWORD).unwrap(),
    )
    .await
    .unwrap();
}

fn get_opds(auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri("/opds/v1");
    if let Some(value) = auth {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    builder.body(Body::empty()).unwrap()
}

fn basic(user: &str, pass: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
    format!("Basic {encoded}")
}

#[tokio::test]
async fn disabled_by_default_returns_503() {
    let app = build_test_app().await;
    // Migration seeds opds_enabled='false'; no configuration done.
    let resp = app
        .request(get_opds(Some(&basic(USERNAME, PASSWORD))))
        .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn enabled_with_no_users_returns_401() {
    let app = build_test_app().await;
    settings_repo::set(&app.state.db, settings_repo::KEY_OPDS_ENABLED, "true")
        .await
        .unwrap();
    // Enabled but no accounts exist: a credentialed request is unauthorized,
    // not 503 (the toggle is the only 503 gate now).
    let resp = app
        .request(get_opds(Some(&basic(USERNAME, PASSWORD))))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn configured_without_credentials_returns_401_with_challenge() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get_opds(None)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Basic realm=\"LongBox\"")
    );
}

#[tokio::test]
async fn correct_basic_credentials_pass_and_stamp_last_seen() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    assert!(opds_users_repo::list(&app.state.db).await.unwrap()[0]
        .last_seen_at
        .is_none());

    let resp = app
        .request(get_opds(Some(&basic(USERNAME, PASSWORD))))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // A successful auth stamps last_seen_at.
    assert!(opds_users_repo::list(&app.state.db).await.unwrap()[0]
        .last_seen_at
        .is_some());
}

#[tokio::test]
async fn username_is_case_insensitive() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app
        .request(get_opds(Some(&basic("READER", PASSWORD))))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn bearer_token_is_rejected() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get_opds(Some("Bearer anything"))).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_password_returns_401() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get_opds(Some(&basic(USERNAME, "wrong")))).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_username_returns_401() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app
        .request(get_opds(Some(&basic("intruder", PASSWORD))))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn disabled_account_returns_401() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let id = opds_users_repo::list(&app.state.db).await.unwrap()[0].id;
    opds_users_repo::set_enabled(&app.state.db, id, false)
        .await
        .unwrap();
    let resp = app
        .request(get_opds(Some(&basic(USERNAME, PASSWORD))))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
