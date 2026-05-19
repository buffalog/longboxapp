//! Integration tests for the Newznab client's HTTP orchestration —
//! `search_indexer` and `find_release` — against wiremock servers
//! serving canned Newznab responses.

use longbox_newznab::{
    find_release, search_indexer, IndexerConfig, IndexerError, IndexerId, NewznabError,
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A Newznab RSS response with one item per (title, guid, grabs).
fn rss(items: &[(&str, &str, i64)]) -> String {
    let mut body = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
<channel>"#,
    );
    for (title, guid, grabs) in items {
        body.push_str(&format!(
            r#"<item>
              <title>{title}</title>
              <guid>{guid}</guid>
              <pubDate>Mon, 05 May 2025 14:30:00 +0000</pubDate>
              <enclosure url="https://dl.example.com/nzb/{guid}"/>
              <newznab:attr name="grabs" value="{grabs}"/>
            </item>"#
        ));
    }
    body.push_str("</channel></rss>");
    body
}

fn empty_rss() -> String {
    rss(&[])
}

fn indexer(id: i64, name: &str, base_url: &str, priority: i32) -> IndexerConfig {
    IndexerConfig {
        id: IndexerId(id),
        name: name.into(),
        base_url: base_url.into(),
        api_key: "TESTKEY".into(),
        priority,
        maxage_days: 1500,
    }
}

#[tokio::test]
async fn search_indexer_parses_a_valid_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "Wolverine 005.cbz",
            "g1",
            10,
        )])))
        .mount(&server)
        .await;

    let idx = indexer(1, "test", &server.uri(), 0);
    let results = search_indexer(&idx, "Wolverine 005").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Wolverine 005.cbz");
    assert_eq!(results[0].nzb_url, "https://dl.example.com/nzb/g1");
}

#[tokio::test]
async fn find_release_returns_best_from_first_indexer_with_results() {
    // Two indexers. Lower priority number is queried first; it has
    // results, so the second is never consulted.
    let first = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[
            ("Wolverine 005 weak.cbz", "weak", 2),
            ("Wolverine 005 strong.cbz", "strong", 99),
        ])))
        .mount(&first)
        .await;

    let second = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "Wolverine 005 from-second.cbz",
            "second",
            500,
        )])))
        .mount(&second)
        .await;

    let indexers = vec![
        indexer(1, "first", &first.uri(), 0),
        indexer(2, "second", &second.uri(), 1),
    ];
    let chosen = find_release(&indexers, "Wolverine", "5", None)
        .await
        .unwrap()
        .expect("a release");
    // Best within the first indexer (highest grabs), and the second
    // indexer's even-higher-grab release is ignored — first wins.
    assert_eq!(chosen.title, "Wolverine 005 strong.cbz");
}

#[tokio::test]
async fn find_release_respects_priority_order() {
    // The indexer with the lower priority number wins even when it's
    // passed second in the slice.
    let low_pri = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "from-priority-0.cbz",
            "p0",
            1,
        )])))
        .mount(&low_pri)
        .await;

    let high_pri = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "from-priority-9.cbz",
            "p9",
            1,
        )])))
        .mount(&high_pri)
        .await;

    // Passed high-priority-number first; find_release must still sort.
    let indexers = vec![
        indexer(2, "high", &high_pri.uri(), 9),
        indexer(1, "low", &low_pri.uri(), 0),
    ];
    let chosen = find_release(&indexers, "X", "1", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(chosen.title, "from-priority-0.cbz");
}

#[tokio::test]
async fn find_release_retries_unpadded_when_padded_is_empty() {
    // Padded query "Wolverine 005" → zero results; unpadded
    // "Wolverine 5" → a hit. The variation retry must kick in.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "Wolverine 005"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "Wolverine 5"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "Wolverine 5 unpadded.cbz",
            "u1",
            7,
        )])))
        .mount(&server)
        .await;

    let indexers = vec![indexer(1, "test", &server.uri(), 0)];
    let chosen = find_release(&indexers, "Wolverine", "5", None)
        .await
        .unwrap()
        .expect("unpadded retry should find it");
    assert_eq!(chosen.title, "Wolverine 5 unpadded.cbz");
}

#[tokio::test]
async fn find_release_is_ok_none_when_all_indexers_return_zero() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss()))
        .mount(&server)
        .await;

    let indexers = vec![indexer(1, "test", &server.uri(), 0)];
    let result = find_release(&indexers, "Nonexistent Series", "1", None)
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn find_release_all_errored_returns_structured_failure() {
    // Two indexers: one HTTP 500 (transient), one bad-credentials
    // (permanent). AllIndexersFailed carries both, distinguishable.
    let down = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&down)
        .await;

    let bad_key = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"<error code="100" description="Incorrect user credentials"/>"#),
        )
        .mount(&bad_key)
        .await;

    let indexers = vec![
        indexer(1, "down", &down.uri(), 0),
        indexer(2, "bad-key", &bad_key.uri(), 1),
    ];
    let err = find_release(&indexers, "X", "1", None).await.unwrap_err();
    let NewznabError::AllIndexersFailed(failures) = err;
    assert_eq!(failures.len(), 2);

    let down_failure = &failures
        .iter()
        .find(|(id, _)| *id == IndexerId(1))
        .unwrap()
        .1;
    assert!(matches!(down_failure, IndexerError::HttpFailure(_)));
    assert!(!down_failure.is_permanent());

    let key_failure = &failures
        .iter()
        .find(|(id, _)| *id == IndexerId(2))
        .unwrap()
        .1;
    assert!(matches!(
        key_failure,
        IndexerError::BadCredentials { code: 100, .. }
    ));
    assert!(key_failure.is_permanent());
}

#[tokio::test]
async fn find_release_ok_none_when_some_error_but_others_just_empty() {
    // One indexer errors, the other cleanly returns zero results.
    // Not "all failed" → Ok(None), not Err.
    let down = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&down)
        .await;

    let empty = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss()))
        .mount(&empty)
        .await;

    let indexers = vec![
        indexer(1, "down", &down.uri(), 0),
        indexer(2, "empty", &empty.uri(), 1),
    ];
    let result = find_release(&indexers, "X", "1", None).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn find_release_empty_indexer_list_is_ok_none() {
    assert!(find_release(&[], "X", "1", None).await.unwrap().is_none());
}
