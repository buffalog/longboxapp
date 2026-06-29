//! OPDS global settings admin API: /api/opds/settings — just the enabled
//! toggle plus the catalog-URL info (port + base_url) the UI renders.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{build_test_app, json_request, response_json, TestApp};

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
async fn initial_settings_are_disabled_and_expose_port_and_base_url() {
    let app = build_test_app().await;
    let s = get_settings(&app).await;
    assert_eq!(s["enabled"], false);
    assert_eq!(s["opds_port"], 8096);
    assert_eq!(s["base_url"], "http://opds.test");
    // The old single-credential fields are gone.
    assert!(s.get("username").is_none());
    assert!(s.get("api_token").is_none());
    assert!(s.get("has_password").is_none());
}

#[tokio::test]
async fn put_toggles_enabled() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "PUT",
            "/api/opds/settings",
            serde_json::json!({ "enabled": true }).to_string(),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await["enabled"], true);
    assert_eq!(get_settings(&app).await["enabled"], true);
}
