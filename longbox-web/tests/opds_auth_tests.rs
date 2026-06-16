//! OPDS auth middleware: 503 when unconfigured/disabled, 401 with the
//! Basic challenge when credentials are missing/wrong, passthrough when
//! Basic or Bearer credentials check out. The feed bodies themselves are
//! exercised in later commits; here we only assert the gate's status codes.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::build_test_app;
use longbox_db::settings_repo;

const USERNAME: &str = "reader";
const PASSWORD: &str = "hunter2";
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Flip OPDS on with a known username/password/token.
async fn configure_opds(db: &longbox_db::Pool) {
    settings_repo::set(db, settings_repo::KEY_OPDS_ENABLED, "true")
        .await
        .unwrap();
    settings_repo::set(db, settings_repo::KEY_OPDS_USERNAME, USERNAME)
        .await
        .unwrap();
    settings_repo::set(
        db,
        settings_repo::KEY_OPDS_PASSWORD_HASH,
        &longbox_opds::hash_password(PASSWORD).unwrap(),
    )
    .await
    .unwrap();
    settings_repo::set(db, settings_repo::KEY_OPDS_API_TOKEN, TOKEN)
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
    let resp = app.request(get_opds(Some(&basic(USERNAME, PASSWORD)))).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn enabled_without_username_returns_503() {
    let app = build_test_app().await;
    settings_repo::set(&app.state.db, settings_repo::KEY_OPDS_ENABLED, "true")
        .await
        .unwrap();
    // Username still empty → "not configured".
    let resp = app.request(get_opds(None)).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
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
async fn correct_basic_credentials_pass() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get_opds(Some(&basic(USERNAME, PASSWORD)))).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn correct_bearer_token_passes() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app
        .request(get_opds(Some(&format!("Bearer {TOKEN}"))))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn wrong_password_returns_401() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app
        .request(get_opds(Some(&basic(USERNAME, "wrong"))))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_username_returns_401() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app
        .request(get_opds(Some(&basic("intruder", PASSWORD))))
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_bearer_token_returns_401() {
    let app = build_test_app().await;
    configure_opds(&app.state.db).await;
    let resp = app.request(get_opds(Some("Bearer notthetoken"))).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
