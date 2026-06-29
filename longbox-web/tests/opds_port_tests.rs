//! The dedicated OPDS-only router (port 8096 in production). It must serve
//! /opds/v1/* (auth-gated) and 404 everything else — no admin API surface.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use common::build_test_app;
use longbox_db::{opds_users_repo, settings_repo};
use tower::ServiceExt;

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("GET").uri(uri);
    if let Some(v) = auth {
        b = b.header(header::AUTHORIZATION, v);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn opds_only_router_404s_the_admin_api() {
    let app = build_test_app().await;
    let router = longbox_web::build_opds_router(app.state.clone());
    // Admin/API + frontend paths that the main app serves must NOT exist here.
    for uri in ["/api/health", "/api/opds/users", "/", "/settings"] {
        let resp = router.clone().oneshot(get(uri, None)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{uri} should 404 on the OPDS-only listener"
        );
    }
}

#[tokio::test]
async fn opds_only_router_serves_the_gated_catalog() {
    let app = build_test_app().await;
    // Disabled by default → 503 (proves the OPDS routes ARE mounted + gated).
    let router = longbox_web::build_opds_router(app.state.clone());
    let resp = router.oneshot(get("/opds/v1", None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Enable + create an account, then a valid Basic request passes.
    settings_repo::set(&app.state.db, settings_repo::KEY_OPDS_ENABLED, "true")
        .await
        .unwrap();
    opds_users_repo::create(
        &app.state.db,
        "reader",
        &longbox_opds::hash_password("hunter2").unwrap(),
    )
    .await
    .unwrap();
    let creds = base64::engine::general_purpose::STANDARD.encode("reader:hunter2");
    let router = longbox_web::build_opds_router(app.state.clone());
    let resp = router
        .oneshot(get("/opds/v1", Some(&format!("Basic {creds}"))))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
