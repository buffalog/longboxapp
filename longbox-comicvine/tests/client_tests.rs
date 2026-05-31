//! Integration tests for `ComicVineClient` via `wiremock`. Each test spins up
//! its own MockServer instance so they run in parallel without state bleed.

use std::time::Duration;

use longbox_comicvine::{ComicVineClient, ComicVineClientConfig, CvError};
use pretty_assertions::assert_eq;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    let p = format!("tests/fixtures/{name}");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("fixture {p}: {e}"))
}

fn client_for(server: &MockServer) -> ComicVineClient {
    let config = ComicVineClientConfig {
        api_key: "test-key".into(),
        // wiremock's URI doesn't include a trailing slash; the client expects
        // base_url to end in `/` so relative paths join cleanly.
        base_url: format!("{}/", server.uri()),
        timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(1),
        // Massive rate so the limiter is effectively a no-op in tests.
        rate_limit_per_hour: 360_000,
        max_wait_for_slot: Duration::from_secs(1),
        user_agent: "longbox-test/0.0".into(),
    };
    ComicVineClient::new(config).expect("test client construction")
}

// ---------- search_volumes ----------

#[tokio::test]
async fn search_volumes_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .and(query_param("resources", "volume"))
        .and(query_param("query", "walking dead"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("search_walking_dead.json")),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let results = client.search_volumes("walking dead").await.unwrap();
    assert_eq!(results.len(), 2);

    let first = &results[0];
    assert_eq!(first.cv_id, 2127);
    assert_eq!(first.name, "The Walking Dead");
    assert_eq!(first.start_year, Some(2003));
    assert_eq!(first.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(first.issue_count, 193);
    assert!(first.cover_url.as_deref().unwrap().contains("scale_medium"));
    assert!(first.description.is_some());

    // Second item exercises null fields → None.
    let second = &results[1];
    assert!(second.cover_url.is_none());
    assert!(second.description.is_none());
}

#[tokio::test]
async fn search_volumes_empty_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("search_empty.json")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let results = client.search_volumes("zzz nothing matches").await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn search_volumes_malformed_result_item() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("search_malformed_result.json")),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.search_volumes("anything").await.unwrap_err();
    match err {
        CvError::Malformed {
            message,
            raw_excerpt,
        } => {
            assert!(
                message.contains("JSON parse error"),
                "expected JSON parse error, got message: {message}"
            );
            assert!(raw_excerpt.is_some());
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

// ---------- fetch_volume ----------

#[tokio::test]
async fn fetch_volume_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-2127/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("volume_4050_2127.json")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let v = client.fetch_volume(2127).await.unwrap();
    assert_eq!(v.cv_id, 2127);
    assert_eq!(v.name, "The Walking Dead");
    assert_eq!(v.start_year, Some(2003));
    assert_eq!(v.publisher.as_deref(), Some("Image Comics"));
    assert!(v.description.as_deref().unwrap().contains("Apocalyptic"));
    assert_eq!(
        v.site_detail_url,
        "https://comicvine.gamespot.com/the-walking-dead/4050-2127/"
    );
}

#[tokio::test]
async fn fetch_volume_returns_not_found_for_status_code_101() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-9999999/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("volume_404.json")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.fetch_volume(9_999_999).await.unwrap_err();
    assert!(matches!(err, CvError::NotFound), "got {err:?}");
}

// ---------- fetch_issues ----------

#[tokio::test]
async fn fetch_issues_single_page() {
    // Single-page case: returned == total → loop exits without a second
    // request. Inline body keeps the test self-contained.
    let body = r#"{
        "status_code": 1,
        "error": "OK",
        "limit": 100,
        "offset": 0,
        "number_of_page_results": 2,
        "number_of_total_results": 2,
        "results": [
            {
                "id": 20001,
                "issue_number": "1",
                "name": null,
                "cover_date": "2012-03-14",
                "description": null,
                "image": null,
                "site_detail_url": "https://cv/saga-1/4000-20001/"
            },
            {
                "id": 20002,
                "issue_number": "2",
                "name": null,
                "cover_date": "2012-04-18",
                "description": null,
                "image": null,
                "site_detail_url": "https://cv/saga-2/4000-20002/"
            }
        ]
    }"#;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .and(query_param("filter", "volume:18166"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        // Crash if anything tries to hit it twice.
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server);
    let issues = client.fetch_issues(18166).await.unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].cv_issue_id, 20001);
    assert_eq!(issues[1].issue_number, "2");
}

#[tokio::test]
async fn fetch_issues_zero_issues() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("issues_empty.json")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let issues = client.fetch_issues(2127).await.unwrap();
    assert!(issues.is_empty());
}

#[tokio::test]
async fn fetch_issues_paginates_and_concatenates() {
    let server = MockServer::start().await;

    // Page 1: offset=0
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .and(query_param("filter", "volume:2127"))
        .and(query_param("offset", "0"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("issues_4050_2127_page_1.json")),
        )
        .mount(&server)
        .await;

    // Page 2: offset=2 (page_1 returned 2 of 3)
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .and(query_param("filter", "volume:2127"))
        .and(query_param("offset", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("issues_4050_2127_page_2.json")),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let issues = client.fetch_issues(2127).await.unwrap();
    assert_eq!(issues.len(), 3);
    assert_eq!(issues[0].issue_number, "1");
    assert_eq!(issues[0].cv_issue_id, 10001);
    assert_eq!(issues[1].issue_number, "2");
    assert_eq!(issues[2].issue_number, "3");
    assert_eq!(issues[2].cv_issue_id, 10003);
}

// ---------- auth / status codes ----------

#[tokio::test]
async fn auth_failure_on_http_401() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(401).set_body_string(fixture("auth_failure.json")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.search_volumes("anything").await.unwrap_err();
    assert!(matches!(err, CvError::Auth), "got {err:?}");
}

#[tokio::test]
async fn auth_failure_on_envelope_status_code_100() {
    // CV's other path for invalid API key: HTTP 200 but envelope status_code 100.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("auth_failure.json")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.search_volumes("anything").await.unwrap_err();
    assert!(matches!(err, CvError::Auth), "got {err:?}");
}

// ---------- 429 retry ----------

#[tokio::test]
async fn rate_limited_429_retries_once_and_succeeds() {
    let server = MockServer::start().await;
    // First call: 429 with Retry-After: 0 (immediate retry).
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Subsequent: success.
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("search_walking_dead.json")),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let results = client.search_volumes("walking dead").await.unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn rate_limited_429_twice_surfaces_as_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.search_volumes("walking dead").await.unwrap_err();
    assert!(matches!(err, CvError::RateLimited { .. }), "got {err:?}");
}

// ---------- 5xx / timeout / unparseable ----------

#[tokio::test]
async fn http_503_surfaces_as_http_variant() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(503).set_body_string(fixture("server_error_503.json")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.search_volumes("anything").await.unwrap_err();
    match err {
        CvError::Http { status, body } => {
            assert_eq!(status, 503);
            assert!(body.contains("Service unavailable"));
        }
        other => panic!("expected Http, got {other:?}"),
    }
}

#[tokio::test]
async fn network_timeout_surfaces_as_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        // 10s delay vs 2s client timeout → fires.
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(fixture("search_empty.json"))
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.search_volumes("anything").await.unwrap_err();
    assert!(matches!(err, CvError::Timeout), "got {err:?}");
}

#[tokio::test]
async fn completely_unparseable_body_surfaces_as_malformed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("malformed.txt")))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.search_volumes("anything").await.unwrap_err();
    match err {
        CvError::Malformed { raw_excerpt, .. } => {
            assert!(raw_excerpt.is_some(), "should attach raw_excerpt");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

// ---------- fetch_release_calendar ----------

#[tokio::test]
async fn fetch_release_calendar_projects_dated_issues() {
    // Three issues come back; the one with no `store_date` is dropped by
    // the projection. The `store_date:from|to` filter is asserted too.
    let body = r#"{
        "status_code": 1,
        "error": "OK",
        "limit": 100,
        "offset": 0,
        "number_of_page_results": 3,
        "number_of_total_results": 3,
        "results": [
            {
                "id": 30001,
                "issue_number": "12",
                "name": null,
                "cover_date": "2026-07-01",
                "store_date": "2026-05-13",
                "description": null,
                "image": { "medium_url": "https://cdn/a.jpg" },
                "volume": { "id": 4050, "name": "Saga" },
                "site_detail_url": "https://cv/4000-30001/"
            },
            {
                "id": 30002,
                "issue_number": "5",
                "name": null,
                "cover_date": "2026-07-01",
                "store_date": "2026-05-14",
                "description": null,
                "image": null,
                "volume": { "id": 6000, "name": "Chew" },
                "site_detail_url": "https://cv/4000-30002/"
            },
            {
                "id": 30003,
                "issue_number": "1",
                "name": null,
                "cover_date": null,
                "store_date": null,
                "description": null,
                "image": null,
                "volume": { "id": 7000, "name": "No Date" },
                "site_detail_url": "https://cv/4000-30003/"
            }
        ]
    }"#;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .and(query_param("filter", "store_date:2026-05-13|2026-05-19"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server);
    let items = client
        .fetch_release_calendar("2026-05-13", "2026-05-19")
        .await
        .unwrap();
    // The store_date-less issue is dropped; the two dated ones project.
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].cv_issue_id, 30001);
    assert_eq!(items[0].store_date, "2026-05-13");
    assert_eq!(items[0].cv_volume_id, 4050);
    assert_eq!(items[0].volume_name, "Saga");
    assert_eq!(items[1].volume_name, "Chew");
}
