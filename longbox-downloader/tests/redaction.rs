//! A submit error must never carry a credential: SAB's request URL has
//! its apikey, and the NZB link has the indexer's key and `r=` token.
//! These failed live on 2026-10-28 (I, Tyrant #2's row showed both keys
//! on Needs Attention).

use longbox_downloader::{Downloader, DownloaderError, NzbgetClient, SabnzbdClient};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const SAB_KEY: &str = "00112233445566778899aabbccddeeff";
const NZB_URL: &str = "https://indexer.example/getnzb/0123456789abcdef0123456789abcdef.nzb\
                       &i=61582&r=fedcba9876543210fedcba9876543210";

/// Fails on anything that looks like a URL, an apikey, or either key.
fn assert_clean(e: &DownloaderError) {
    for text in [e.to_string(), format!("{e:?}")] {
        let lower = text.to_lowercase();
        assert!(!lower.contains("apikey"), "apikey in: {text}");
        assert!(
            !text.contains("://") && !lower.contains("%3a%2f%2f"),
            "URL in: {text}"
        );
        for secret in [
            SAB_KEY,
            "0123456789abcdef0123456789abcdef",
            "fedcba9876543210",
        ] {
            assert!(!text.contains(secret), "secret in: {text}");
        }
    }
}

/// A port nothing listens on: reqwest's error names the full request URL.
async fn dead_base() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

#[tokio::test]
async fn sab_submit_errors_carry_no_url_or_key() {
    // Transport failure: the live I, Tyrant #2 shape.
    let sab = SabnzbdClient::new(dead_base().await, SAB_KEY.into(), "comics".into());
    let e = sab.submit(NZB_URL, "I, Tyrant 2").await.unwrap_err();
    assert!(matches!(e, DownloaderError::HttpFailure(_)), "{e}");
    assert_clean(&e);

    // SAB's own error text echoing what it was asked to fetch.
    let server = MockServer::start().await;
    let echoed =
        format!(r#"{{"status":false,"error":"Failed to fetch {NZB_URL}?apikey={SAB_KEY}"}}"#);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(echoed))
        .mount(&server)
        .await;
    let sab = SabnzbdClient::new(server.uri(), SAB_KEY.into(), "comics".into());
    let e = sab.submit(NZB_URL, "I, Tyrant 2").await.unwrap_err();
    assert!(matches!(e, DownloaderError::ApiError(_)), "{e}");
    assert_clean(&e);
}

#[tokio::test]
async fn nzbget_submit_errors_carry_no_url_or_key() {
    // Credentials in the base URL, as some setups configure it.
    let base = dead_base()
        .await
        .replace("http://", &format!("http://user:{SAB_KEY}@"));
    let nzbget = NzbgetClient::new(base, "user".into(), SAB_KEY.into(), "comics".into());
    let e = nzbget.submit(NZB_URL, "I, Tyrant 2").await.unwrap_err();
    assert_clean(&e);

    let server = MockServer::start().await;
    let echoed = serde_json::json!({
        "result": null,
        "error": { "code": 1, "message": format!("could not fetch {NZB_URL}") },
    });
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(echoed))
        .mount(&server)
        .await;
    let nzbget = NzbgetClient::new(server.uri(), "user".into(), "pw".into(), "comics".into());
    let e = nzbget.submit(NZB_URL, "I, Tyrant 2").await.unwrap_err();
    assert!(matches!(e, DownloaderError::ApiError(_)), "{e}");
    assert_clean(&e);
}
