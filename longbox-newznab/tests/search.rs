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
async fn the_bare_rung_is_reached_when_the_narrow_one_returns_nothing() {
    // The Brother Lono shape: the narrow rung matches nothing, because
    // newznab `q` is a crude substring match over dot-separated scene
    // names. The bare rung has it. The mock answers ONLY the bare term,
    // so a ladder that stopped at the narrow rung would find nothing.
    let server = MockServer::start().await;
    // Narrow rung: a clean empty response, not an error — the shape a
    // real indexer returns when the crude substring match finds nothing.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "wolverine 005"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "wolverine"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "Wolverine 005 (2024).cbz",
            "u1",
            7,
        )])))
        .mount(&server)
        .await;

    let indexers = vec![indexer(1, "test", &server.uri(), 0)];
    let chosen = find_release(&indexers, "Wolverine", "5", None)
        .await
        .unwrap()
        .expect("the series-only query must find it");
    assert_eq!(chosen.title, "Wolverine 005 (2024).cbz");
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

/// Integration bar for the underscore-separated indexer title.
///
/// Some indexers (DrunkenSlug) normalize every non-alphanumeric
/// character in a release name to `_`. The release below is the same
/// one NZBGeek ships as
/// `The Author Immortal 005 [2026] [Digital] [Zone-Empire]`.
///
/// This exercises the REAL pull-engine entrypoint — HTTP fetch, parse,
/// pre-grab similarity gate at the production 0.75 threshold, and
/// selection — not just the normalizer in isolation. Pre-fix the
/// cascade could not parse the title, so the gate counted it
/// unparseable and returned `Mismatch` (logged as series_mismatch),
/// declining a release that was in stock.
#[tokio::test]
async fn find_release_filtered_accepts_underscore_separated_title() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "The_Author_Immortal_005__2026___Digital___Zone-Empire_",
            "underscored",
            7,
        )])))
        .mount(&server)
        .await;

    let indexers = vec![indexer(1, "drunkenslug", &server.uri(), 0)];
    let outcome = longbox_newznab::find_release_excluding_filtered(
        &indexers,
        "The Author Immortal",
        "5",
        Some(2026),
        &[],
        &longbox_core::filename::default_patterns(),
        0.75,
        &[],
        None,
        None,
        &[],
    )
    .await
    .unwrap();

    match outcome {
        longbox_newznab::FindOutcome::Match { release, .. } => {
            assert_eq!(
                release.title,
                "The_Author_Immortal_005__2026___Digital___Zone-Empire_"
            );
        }
        other => panic!("expected the underscored release to be grabbable, got {other:?}"),
    }
}

/// The ladder must not declare victory on results the engine rejects.
///
/// The defect needs the WRONG rung to come FIRST — otherwise removing
/// the issue number alone would mask it. So: the full-title rung
/// returns a different book, and the ALIAS rung has the right release.
/// (An earlier version of this test put the right release on rung 0,
/// which the ladder reaches first regardless of the success rule; it
/// passed against the old "any HTTP results wins" behaviour and proved
/// nothing. Mutation testing caught it.)
#[tokio::test]
async fn a_rung_returning_only_wrong_series_does_not_stop_the_ladder() {
    let server = MockServer::start().await;
    // Rung 0 — precision term. Returns a different book entirely.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "fbp federal bureau of physics 001"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "100.Bullets.001.1999.G85.and.Megan-Empire",
            "wrong-0",
            99,
        )])))
        .mount(&server)
        .await;
    // Rung 1 — bare full title. Also wrong.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "fbp federal bureau of physics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "100.Bullets.001.1999.G85.and.Megan-Empire",
            "wrong-1",
            99,
        )])))
        .mount(&server)
        .await;
    // Rung 1 — after-colon. Also wrong.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "federal bureau of physics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "Some.Other.Book.002.2020.Digital-Empire",
            "wrong-2",
            50,
        )])))
        .mount(&server)
        .await;
    // before-colon rung — now LAST, and empty. Mocked explicitly so the
    // test exercises the survival rule, not wiremock's 404 path.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "fbp"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_rss()))
        .mount(&server)
        .await;
    // Alias rung — the right one, reached only if the ladder keeps going.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("q", "collider"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "FBP.-.Federal.Bureau.of.Physics.001.2013.Digital-Empire",
            "right-1",
            3,
        )])))
        .mount(&server)
        .await;

    let indexers = vec![indexer(1, "test", &server.uri(), 0)];
    let outcome = longbox_newznab::find_release_excluding_filtered(
        &indexers,
        "FBP: Federal Bureau of Physics",
        "1",
        None,
        &[],
        &longbox_core::filename::default_patterns(),
        longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        &[],
        Some("2013-09-01"),
        None,
        &["Collider".to_string()],
    )
    .await
    .unwrap();

    match outcome {
        longbox_newznab::FindOutcome::Match { release, .. } => assert_eq!(
            release.title, "FBP.-.Federal.Bureau.of.Physics.001.2013.Digital-Empire",
            "a wrong-series rung must not stop the ladder"
        ),
        other => panic!("expected the alias rung's release, got {other:?}"),
    }
}

/// An exhausted ladder must still hand back what it saw, so the caller
/// can raise a `Mismatch` ("N returned, none matched") instead of a
/// silent `NoMatch`. Returning an empty pool here would convert a
/// surfaced failure into an invisible one — the same trade this change
/// exists to stop making, reintroduced at a different layer.
#[tokio::test]
async fn an_exhausted_ladder_still_reports_a_mismatch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss(&[(
            "Some.Totally.Different.Book.001.2020.Digital-Empire",
            "wrong-1",
            5,
        )])))
        .mount(&server)
        .await;

    let indexers = vec![indexer(1, "test", &server.uri(), 0)];
    let outcome = longbox_newznab::find_release_excluding_filtered(
        &indexers,
        "Saga",
        "1",
        None,
        &[],
        &longbox_core::filename::default_patterns(),
        longbox_core::PULL_INDEXER_MATCH_THRESHOLD,
        &[],
        None,
        None,
        &[],
    )
    .await
    .unwrap();

    match outcome {
        longbox_newznab::FindOutcome::Mismatch { diagnostic, .. } => {
            assert_eq!(diagnostic.total_results, 1);
        }
        other => panic!("exhaustion must stay diagnosable, got {other:?}"),
    }
}
