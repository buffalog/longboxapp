//! OPDS user account admin API: /api/opds/users CRUD.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::{build_test_app, json_request, response_json, TestApp};
use longbox_db::settings_repo;

async fn create(app: &TestApp, body: serde_json::Value) -> axum::response::Response {
    app.request(json_request("POST", "/api/opds/users", body.to_string()))
        .await
}

async fn list(app: &TestApp) -> serde_json::Value {
    let resp = app
        .request(
            Request::builder()
                .method("GET")
                .uri("/api/opds/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    response_json(resp).await
}

#[tokio::test]
async fn create_then_list_returns_the_account_without_a_hash() {
    let app = build_test_app().await;
    let resp = create(
        &app,
        serde_json::json!({ "username": "Judd", "password": "hunter2pw" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = response_json(resp).await;
    assert_eq!(created["username"], "Judd");
    assert_eq!(created["enabled"], true);
    // The hash must never be serialized.
    assert!(created.get("password_hash").is_none());

    let users = list(&app).await;
    assert_eq!(users.as_array().unwrap().len(), 1);
    assert_eq!(users[0]["username"], "Judd");
    assert!(users[0]["last_seen_at"].is_null());
}

#[tokio::test]
async fn create_rejects_empty_username_and_short_password() {
    let app = build_test_app().await;
    let resp = create(
        &app,
        serde_json::json!({ "username": "  ", "password": "longenough" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = create(
        &app,
        serde_json::json!({ "username": "cody", "password": "short" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn duplicate_username_is_a_conflict() {
    let app = build_test_app().await;
    create(
        &app,
        serde_json::json!({ "username": "reader", "password": "hunter2pw" }),
    )
    .await;
    // Case-insensitive: "Reader" collides with "reader".
    let resp = create(
        &app,
        serde_json::json!({ "username": "Reader", "password": "another1" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn disable_then_enable_round_trips_and_gates_the_catalog() {
    let app = build_test_app().await;
    settings_repo::set(&app.state.db, settings_repo::KEY_OPDS_ENABLED, "true")
        .await
        .unwrap();
    let id = response_json(
        create(
            &app,
            serde_json::json!({ "username": "reader", "password": "hunter2pw" }),
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();

    let creds = base64::engine::general_purpose::STANDARD.encode("reader:hunter2pw");
    let catalog = || {
        Request::builder()
            .method("GET")
            .uri("/opds/v1")
            .header(header::AUTHORIZATION, format!("Basic {creds}"))
            .body(Body::empty())
            .unwrap()
    };

    // Enabled account authorizes.
    assert_eq!(app.request(catalog()).await.status(), StatusCode::OK);

    // Disable → catalog rejects.
    let resp = app
        .request(json_request(
            "POST",
            &format!("/api/opds/users/{id}/disable"),
            "",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        app.request(catalog()).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // Re-enable → authorizes again.
    let resp = app
        .request(json_request(
            "POST",
            &format!("/api/opds/users/{id}/enable"),
            "",
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(app.request(catalog()).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_removes_the_account() {
    let app = build_test_app().await;
    let id = response_json(
        create(
            &app,
            serde_json::json!({ "username": "temp", "password": "hunter2pw" }),
        )
        .await,
    )
    .await["id"]
        .as_i64()
        .unwrap();

    let resp = app
        .request(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/opds/users/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(list(&app).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn mutating_an_unknown_id_is_404() {
    let app = build_test_app().await;
    for (method, uri) in [
        ("POST", "/api/opds/users/999/enable"),
        ("POST", "/api/opds/users/999/disable"),
    ] {
        let resp = app.request(json_request(method, uri, "")).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{method} {uri}");
    }
    let resp = app
        .request(
            Request::builder()
                .method("DELETE")
                .uri("/api/opds/users/999")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
