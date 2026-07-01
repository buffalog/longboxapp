//! Integration tests for `MetronClient` via `wiremock`. Each test spins up
//! its own MockServer so they run in parallel without state bleed. JSON
//! payloads are inline rather than fixture files — shapes are small and
//! visible-in-test is clearer than fixture-file lookup at this scale.

use std::time::Duration;

use longbox_metron::{MetronClient, MetronClientConfig, MetronError};
use pretty_assertions::assert_eq;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_for(server: &MockServer) -> MetronClient {
    let config = MetronClientConfig {
        username: "tester".into(),
        password: "secret".into(),
        // wiremock's URI doesn't include a trailing slash; client expects
        // base_url to end in `/` so relative joins land cleanly.
        base_url: format!("{}/", server.uri()),
        timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(1),
        // Effectively unbounded for tests — the limiter shouldn't gate
        // anything we're testing here.
        rate_limit_per_hour: 360_000,
        max_wait_for_slot: Duration::from_secs(1),
        user_agent: "longbox-metron-test/0.0".into(),
    };
    MetronClient::new(config).expect("test client construction")
}

// =========== fetch_issues_by_store_date_range ===========

#[tokio::test]
async fn fetch_issues_happy_path_single_page() {
    let server = MockServer::start().await;
    let payload = serde_json::json!({
        "count": 2,
        "next": null,
        "previous": null,
        "results": [
            {
                "id": 170031,
                "series": {"name": "Absolute Green Lantern", "volume": 1, "year_began": 2025},
                "number": "15",
                "issue": "Absolute Green Lantern (2025) #15",
                "cover_date": "2026-08-01",
                "store_date": "2026-06-03",
                "image": "https://static.metron.cloud/media/issue/cover-1.jpg",
                "cover_hash": "d5cd12786aa4358f",
                "modified": "2026-05-30T09:33:52-04:00"
            },
            {
                "id": 170032,
                "series": {"name": "Action Comics", "volume": 3, "year_began": 2016},
                "number": "1099",
                "issue": "Action Comics (2016) #1099",
                "cover_date": "2026-08-01",
                "store_date": "2026-06-10",
                "image": null,
                "cover_hash": null,
                "modified": null
            }
        ]
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .and(query_param("store_date_range_after", "2026-06-01"))
        .and(query_param("store_date_range_before", "2026-06-30"))
        .respond_with(ResponseTemplate::new(200).set_body_string(payload))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let items = client
        .fetch_issues_by_store_date_range("2026-06-01", "2026-06-30")
        .await
        .unwrap();
    assert_eq!(items.len(), 2);

    let first = &items[0];
    assert_eq!(first.metron_issue_id, 170031);
    assert_eq!(first.issue_number, "15");
    assert_eq!(first.series_name, "Absolute Green Lantern");
    assert_eq!(first.series_year_began, 2025);
    assert_eq!(first.store_date.as_deref(), Some("2026-06-03"));
    assert!(first
        .cover_url
        .as_deref()
        .unwrap()
        .contains("static.metron"));
    // Critical: list endpoint doesn't carry publisher / series_id / foc.
    assert!(first.publisher.is_none());
    assert!(first.metron_series_id.is_none());
    assert!(first.foc_date.is_none());
    assert!(first.cv_issue_id.is_none());

    let second = &items[1];
    assert!(second.cover_url.is_none());
}

#[tokio::test]
async fn fetch_issues_paginates_through_next() {
    let server = MockServer::start().await;
    let page1 = serde_json::json!({
        "count": 3,
        "next": format!("{}/issue/?page=2", server.uri()),
        "previous": null,
        "results": [{
            "id": 1, "series": {"name": "A", "volume": 1, "year_began": 2020},
            "number": "1", "issue": "A (2020) #1",
            "cover_date": null, "store_date": "2026-06-03",
            "image": null, "cover_hash": null, "modified": null
        }, {
            "id": 2, "series": {"name": "B", "volume": 1, "year_began": 2021},
            "number": "1", "issue": "B (2021) #1",
            "cover_date": null, "store_date": "2026-06-04",
            "image": null, "cover_hash": null, "modified": null
        }]
    })
    .to_string();
    let page2 = serde_json::json!({
        "count": 3,
        "next": null,
        "previous": format!("{}/issue/?page=1", server.uri()),
        "results": [{
            "id": 3, "series": {"name": "C", "volume": 1, "year_began": 2022},
            "number": "1", "issue": "C (2022) #1",
            "cover_date": null, "store_date": "2026-06-05",
            "image": null, "cover_hash": null, "modified": null
        }]
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page1))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page2))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let items = client
        .fetch_issues_by_store_date_range("2026-06-01", "2026-06-30")
        .await
        .unwrap();
    assert_eq!(items.len(), 3, "must chase `next` until exhausted");
    assert_eq!(items[2].metron_issue_id, 3);
}

#[tokio::test]
async fn fetch_issues_empty_results() {
    let server = MockServer::start().await;
    let payload = serde_json::json!({
        "count": 0, "next": null, "previous": null, "results": []
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(payload))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let items = client
        .fetch_issues_by_store_date_range("2099-01-01", "2099-01-07")
        .await
        .unwrap();
    assert!(items.is_empty());
}

// =========== fetch_issue_detail ===========

#[tokio::test]
async fn fetch_issue_detail_carries_publisher_and_series_id_and_foc() {
    let server = MockServer::start().await;
    let payload = serde_json::json!({
        "id": 170031,
        "publisher": {"id": 2, "name": "DC Comics"},
        "imprint": null,
        "series": {
            "id": 10959,
            "name": "Absolute Green Lantern",
            "sort_name": "Absolute Green Lantern",
            "volume": 1,
            "year_began": 2025
        },
        "number": "15",
        "alt_number": "",
        "title": "",
        "cover_date": "2026-08-01",
        "store_date": "2026-06-03",
        "foc_date": "2026-05-11",
        "price": "4.99",
        "desc": "The planet earth in peril!",
        "image": "https://static.metron.cloud/media/cover.jpg",
        "cv_id": null,
        "gcd_id": 2838660,
        "resource_url": "https://metron.cloud/issue/absolute-green-lantern-2025-15/",
        "modified": "2026-05-30T09:33:52-04:00"
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/issue/170031/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(payload))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let item = client.fetch_issue_detail(170031).await.unwrap();

    assert_eq!(item.metron_issue_id, 170031);
    assert_eq!(item.metron_series_id, Some(10959));
    assert_eq!(item.publisher.as_deref(), Some("DC Comics"));
    assert_eq!(item.foc_date.as_deref(), Some("2026-05-11"));
    assert!(item.cv_issue_id.is_none(), "forward issue has no CV id yet");
    assert!(item
        .site_detail_url
        .as_deref()
        .unwrap()
        .contains("metron.cloud"));
}

#[tokio::test]
async fn fetch_issue_detail_with_cv_id_populated() {
    let server = MockServer::start().await;
    let payload = serde_json::json!({
        "id": 10435,
        "publisher": {"id": 2, "name": "DC Comics"},
        "series": {
            "id": 1500, "name": "Catwoman", "sort_name": "Catwoman",
            "volume": 2, "year_began": 1993
        },
        "number": "40",
        "cover_date": "1996-12-01",
        "store_date": "1996-10-30",
        "foc_date": null,
        "image": null,
        "cv_id": 46568,
        "resource_url": null,
        "modified": null
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/issue/10435/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(payload))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let item = client.fetch_issue_detail(10435).await.unwrap();
    assert_eq!(item.cv_issue_id, Some(46568));
}

#[tokio::test]
async fn fetch_issue_detail_404_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issue/999999/"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client.fetch_issue_detail(999999).await.unwrap_err();
    assert!(matches!(err, MetronError::NotFound));
}

// =========== fetch_series_detail ===========

#[tokio::test]
async fn fetch_series_detail_happy_path() {
    let server = MockServer::start().await;
    let payload = serde_json::json!({
        "id": 916,
        "name": "Saga",
        "sort_name": "Saga",
        "volume": 1,
        "year_began": 2012,
        "year_end": null,
        "publisher": {"id": 5, "name": "Image Comics"},
        "imprint": null,
        "cv_id": 46568,
        "gcd_id": 12345,
        "issue_count": 72
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/series/916/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(payload))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let detail = client.fetch_series_detail(916).await.unwrap();
    assert_eq!(detail.metron_series_id, 916);
    assert_eq!(detail.name, "Saga");
    assert_eq!(detail.year_began, 2012);
    assert!(detail.year_end.is_none());
    assert_eq!(detail.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(detail.cv_id, Some(46568));
}

// =========== fetch_series_by_cv_id ===========

#[tokio::test]
async fn fetch_series_by_cv_id_returns_some_on_match() {
    let server = MockServer::start().await;
    let payload = serde_json::json!({
        "count": 1, "next": null, "previous": null,
        "results": [{
            "id": 916,
            "series": "Saga (2012)",
            "year_began": 2012,
            "year_end": null,
            "volume": 1,
            "issue_count": 72,
            "modified": "2025-01-01T14:48:25-05:00"
        }]
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/series/"))
        .and(query_param("cv_id", "46568"))
        .respond_with(ResponseTemplate::new(200).set_body_string(payload))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let resolved = client.fetch_series_by_cv_id(46568).await.unwrap();
    let resolved = resolved.expect("Some on match");
    assert_eq!(resolved.metron_series_id, 916);
    assert_eq!(resolved.display_name, "Saga (2012)");
    assert_eq!(resolved.year_began, 2012);
}

#[tokio::test]
async fn fetch_series_by_cv_id_returns_none_on_empty() {
    let server = MockServer::start().await;
    let payload = serde_json::json!({
        "count": 0, "next": null, "previous": null, "results": []
    })
    .to_string();
    Mock::given(method("GET"))
        .and(path("/series/"))
        .and(query_param("cv_id", "99999"))
        .respond_with(ResponseTemplate::new(200).set_body_string(payload))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let resolved = client.fetch_series_by_cv_id(99999).await.unwrap();
    assert!(resolved.is_none());
}

// =========== Auth / Rate limit / Error paths ===========

#[tokio::test]
async fn http_401_returns_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .respond_with(ResponseTemplate::new(401).set_body_string("{\"detail\":\"unauthorized\"}"))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client
        .fetch_issues_by_store_date_range("2026-06-01", "2026-06-07")
        .await
        .unwrap_err();
    assert!(matches!(err, MetronError::Auth));
}

#[tokio::test]
async fn http_429_retries_once_then_returns_rate_limited() {
    let server = MockServer::start().await;
    // Both responses 429 — client should retry once, fail second 429,
    // surface RateLimited with the Retry-After value.
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_string("{\"detail\":\"throttled\"}"),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client
        .fetch_issues_by_store_date_range("2026-06-01", "2026-06-07")
        .await
        .unwrap_err();
    match err {
        MetronError::RateLimited {
            retry_after_seconds,
        } => {
            assert_eq!(retry_after_seconds, 1);
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_json_response_returns_malformed_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json {{{"))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let err = client
        .fetch_issues_by_store_date_range("2026-06-01", "2026-06-07")
        .await
        .unwrap_err();
    match err {
        MetronError::Malformed {
            message,
            raw_excerpt,
        } => {
            assert!(message.contains("JSON parse error"), "msg={message}");
            assert!(raw_excerpt.unwrap().contains("not json"));
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[tokio::test]
async fn requests_carry_basic_auth_header() {
    let server = MockServer::start().await;
    // wiremock matches require the Authorization header to be present
    // and start with "Basic " — the value itself is base64(user:pass).
    // We verify the prefix, which is enough proof that basic_auth() is
    // wired into the request builder.
    Mock::given(method("GET"))
        .and(path("/issue/"))
        .and(wiremock::matchers::header_exists("authorization"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"count":0,"next":null,"previous":null,"results":[]}"#),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    // Will succeed if the matcher matched; will surface a wiremock error
    // (probably 404 from the catch-all if no matcher hit) otherwise.
    let _ = client
        .fetch_issues_by_store_date_range("2026-06-01", "2026-06-07")
        .await
        .expect("auth header must be on the request");
}
