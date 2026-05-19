//! Integration tests for the SABnzbd client against wiremock.

use longbox_downloader::{
    connect, DownloadHandle, DownloadStatus, Downloader, DownloaderAuth, DownloaderConfig,
    DownloaderError,
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sab(base_url: &str) -> impl Downloader {
    connect(&DownloaderConfig {
        base_url: base_url.into(),
        auth: DownloaderAuth::ApiKey("TESTKEY".into()),
        category: "comics".into(),
    })
}

#[tokio::test]
async fn submit_returns_the_nzo_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "addurl"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"status": true, "nzo_ids": ["SABnzbd_nzo_abc123"]}"#),
        )
        .mount(&server)
        .await;

    let handle = sab(&server.uri())
        .submit("https://idx/getnzb/x", "Wolverine 005")
        .await
        .unwrap();
    assert_eq!(handle, DownloadHandle("SABnzbd_nzo_abc123".into()));
}

#[tokio::test]
async fn submit_bad_apikey_is_auth_failed() {
    let server = MockServer::start().await;
    // SABnzbd returns a plain-text body for a bad key — not JSON.
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string("error: API Key Incorrect"))
        .mount(&server)
        .await;

    let err = sab(&server.uri())
        .submit("https://idx/getnzb/x", "X")
        .await
        .unwrap_err();
    assert!(matches!(err, DownloaderError::AuthFailed(_)));
    assert!(err.is_permanent());
}

#[tokio::test]
async fn submit_status_false_is_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"status": false, "error": "nzb fetch failed"}"#),
        )
        .mount(&server)
        .await;

    let err = sab(&server.uri())
        .submit("https://idx/getnzb/x", "X")
        .await
        .unwrap_err();
    assert!(matches!(err, DownloaderError::ApiError(m) if m == "nzb fetch failed"));
}

#[tokio::test]
async fn status_resolves_from_the_active_queue() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"queue": {"slots": [
                {"nzo_id": "SABnzbd_nzo_abc123", "status": "Downloading"}
            ]}}"#,
        ))
        .mount(&server)
        .await;

    let status = sab(&server.uri())
        .status(&DownloadHandle("SABnzbd_nzo_abc123".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Downloading);
}

#[tokio::test]
async fn status_resolves_completed_from_history() {
    let server = MockServer::start().await;
    // Not in the queue...
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue": {"slots": []}}"#))
        .mount(&server)
        .await;
    // ...found in history.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"history": {"slots": [
                {"nzo_id": "SABnzbd_nzo_abc123", "status": "Completed", "fail_message": ""}
            ]}}"#,
        ))
        .mount(&server)
        .await;

    let status = sab(&server.uri())
        .status(&DownloadHandle("SABnzbd_nzo_abc123".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Completed);
}

#[tokio::test]
async fn status_resolves_failed_with_message_from_history() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue": {"slots": []}}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"history": {"slots": [
                {"nzo_id": "nzo_x", "status": "Failed", "fail_message": "Repair failed, not enough blocks"}
            ]}}"#,
        ))
        .mount(&server)
        .await;

    let status = sab(&server.uri())
        .status(&DownloadHandle("nzo_x".into()))
        .await
        .unwrap();
    assert_eq!(
        status,
        DownloadStatus::Failed("Repair failed, not enough blocks".into())
    );
}

#[tokio::test]
async fn status_unknown_when_in_neither_queue_nor_history() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue": {"slots": []}}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"history": {"slots": []}}"#))
        .mount(&server)
        .await;

    let status = sab(&server.uri())
        .status(&DownloadHandle("nzo_ghost".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Unknown);
}
