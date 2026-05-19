//! Integration tests for the NZBGet client against wiremock.

use longbox_downloader::{
    connect, DownloadHandle, DownloadStatus, Downloader, DownloaderAuth, DownloaderConfig,
    DownloaderError,
};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn nzbget(base_url: &str) -> impl Downloader {
    connect(&DownloaderConfig {
        base_url: base_url.into(),
        auth: DownloaderAuth::Basic {
            username: "nzbget".into(),
            password: "tegbzn".into(),
        },
        category: "comics".into(),
    })
}

/// Mount a JSON-RPC method handler returning the given `result`.
async fn mount_method(server: &MockServer, method_name: &str, result: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/jsonrpc"))
        .and(body_partial_json(json!({ "method": method_name })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": "1.1",
            "result": result,
            "id": 1
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn submit_returns_the_nzbid() {
    let server = MockServer::start().await;
    mount_method(&server, "append", json!(42)).await;

    let handle = nzbget(&server.uri())
        .submit("https://idx/getnzb/x", "Wolverine 005")
        .await
        .unwrap();
    assert_eq!(handle, DownloadHandle("42".into()));
}

#[tokio::test]
async fn submit_nonpositive_result_is_api_error() {
    let server = MockServer::start().await;
    // append returns 0 / negative on rejection.
    mount_method(&server, "append", json!(0)).await;

    let err = nzbget(&server.uri())
        .submit("https://idx/getnzb/x", "X")
        .await
        .unwrap_err();
    assert!(matches!(err, DownloaderError::ApiError(_)));
}

#[tokio::test]
async fn submit_jsonrpc_error_envelope_is_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/jsonrpc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": "1.1",
            "error": { "code": -32601, "message": "Method not found" },
            "id": 1
        })))
        .mount(&server)
        .await;

    let err = nzbget(&server.uri())
        .submit("https://idx/getnzb/x", "X")
        .await
        .unwrap_err();
    assert!(matches!(err, DownloaderError::ApiError(m) if m.contains("Method not found")));
}

#[tokio::test]
async fn submit_http_401_is_auth_failed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/jsonrpc"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = nzbget(&server.uri())
        .submit("https://idx/getnzb/x", "X")
        .await
        .unwrap_err();
    assert!(matches!(err, DownloaderError::AuthFailed(_)));
    assert!(err.is_permanent());
}

#[tokio::test]
async fn status_resolves_downloading_from_listgroups() {
    let server = MockServer::start().await;
    mount_method(
        &server,
        "listgroups",
        json!([{ "NZBID": 42, "Status": "DOWNLOADING" }]),
    )
    .await;

    let status = nzbget(&server.uri())
        .status(&DownloadHandle("42".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Downloading);
}

#[tokio::test]
async fn status_resolves_completed_from_history() {
    let server = MockServer::start().await;
    mount_method(&server, "listgroups", json!([])).await;
    mount_method(
        &server,
        "history",
        json!([{ "NZBID": 42, "Status": "SUCCESS/ALL" }]),
    )
    .await;

    let status = nzbget(&server.uri())
        .status(&DownloadHandle("42".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Completed);
}

#[tokio::test]
async fn status_warning_band_maps_to_completed() {
    // WARNING/* is mapped to Completed inside NzbgetClient — the pull
    // engine never sees NZBGet-specific status strings.
    let server = MockServer::start().await;
    mount_method(&server, "listgroups", json!([])).await;
    mount_method(
        &server,
        "history",
        json!([{ "NZBID": 7, "Status": "WARNING/SCRIPT" }]),
    )
    .await;

    let status = nzbget(&server.uri())
        .status(&DownloadHandle("7".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Completed);
}

#[tokio::test]
async fn status_failure_band_maps_to_failed() {
    let server = MockServer::start().await;
    mount_method(&server, "listgroups", json!([])).await;
    mount_method(
        &server,
        "history",
        json!([{ "NZBID": 9, "Status": "FAILURE/PAR" }]),
    )
    .await;

    let status = nzbget(&server.uri())
        .status(&DownloadHandle("9".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Failed("FAILURE/PAR".into()));
}

#[tokio::test]
async fn status_unknown_when_in_neither_listgroups_nor_history() {
    let server = MockServer::start().await;
    mount_method(&server, "listgroups", json!([])).await;
    mount_method(&server, "history", json!([])).await;

    let status = nzbget(&server.uri())
        .status(&DownloadHandle("404".into()))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Unknown);
}
