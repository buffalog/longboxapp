//! OPDS settings admin API: /api/opds/settings (commit 7).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::{build_test_app, json_request, response_json, TestApp};

async fn put(app: &TestApp, body: serde_json::Value) -> serde_json::Value {
    let resp = app
        .request(json_request("PUT", "/api/opds/settings", body.to_string()))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    response_json(resp).await
}

async fn get_settings(app: &TestApp) -> serde_json::Value {
    let resp = app
        .request(
            Request::builder()
                .method("GET")
                .uri("/api/opds/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    response_json(resp).await
}

#[tokio::test]
async fn initial_settings_are_empty_and_disabled() {
    let app = build_test_app().await;
    let s = get_settings(&app).await;
    assert_eq!(s["enabled"], false);
    assert_eq!(s["username"], "");
    assert_eq!(s["has_password"], false);
    assert_eq!(s["api_token"], "");
    assert_eq!(s["catalog_url"], "http://opds.test/opds/v1");
    // The hash must never be serialized.
    assert!(s.get("password_hash").is_none());
}

#[tokio::test]
async fn first_save_generates_token_and_hashes_password() {
    let app = build_test_app().await;
    let s = put(
        &app,
        serde_json::json!({ "enabled": true, "username": "reader", "password": "hunter2" }),
    )
    .await;

    assert_eq!(s["enabled"], true);
    assert_eq!(s["username"], "reader");
    assert_eq!(s["has_password"], true);
    // 32-byte hex token generated on first save.
    let token = s["api_token"].as_str().unwrap();
    assert_eq!(token.len(), 64);
    assert!(s.get("password_hash").is_none());
}

#[tokio::test]
async fn configured_settings_actually_authorize_the_catalog() {
    let app = build_test_app().await;
    put(
        &app,
        serde_json::json!({ "enabled": true, "username": "reader", "password": "hunter2" }),
    )
    .await;

    // Basic auth with the configured creds now reaches the catalog.
    let encoded = base64::engine::general_purpose::STANDARD.encode("reader:hunter2");
    let resp = app
        .request(
            Request::builder()
                .method("GET")
                .uri("/opds/v1")
                .header(header::AUTHORIZATION, format!("Basic {encoded}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // And the generated bearer token works too.
    let token = get_settings(&app).await["api_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = app
        .request(
            Request::builder()
                .method("GET")
                .uri("/opds/v1")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn empty_password_leaves_existing_unchanged() {
    let app = build_test_app().await;
    put(
        &app,
        serde_json::json!({ "username": "reader", "password": "hunter2" }),
    )
    .await;
    // A later save without retyping the password must not clear it.
    let s = put(&app, serde_json::json!({ "username": "reader2", "password": "" })).await;
    assert_eq!(s["username"], "reader2");
    assert_eq!(s["has_password"], true);
}

#[tokio::test]
async fn regenerate_token_changes_it() {
    let app = build_test_app().await;
    let first = put(&app, serde_json::json!({ "username": "reader" })).await["api_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(first.len(), 64);

    let resp = app
        .request(json_request(
            "POST",
            "/api/opds/settings/regenerate-token",
            "",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let second = response_json(resp).await["api_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(second.len(), 64);
    assert_ne!(first, second);
}
