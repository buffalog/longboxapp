//! Interactive Search endpoint: POST /api/search/interactive (commit 1).

mod common;

use axum::http::StatusCode;
use common::{build_test_app, json_request, response_json, TestApp};
use longbox_db::{
    downloader_config_repo, file_repo, indexer_config_repo, issue_repo, library_root_repo,
    series_repo, NewDownloaderConfig, NewFile, NewIndexerConfig, NewIssue, NewLibraryRoot,
    NewSeries,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// RSS with two correct-series releases (cbz + cbr) and one wrong-series
/// release, each carrying a size and pubDate.
const RSS: &str = r#"<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
  <channel>
    <item>
      <title>Saga 012 (2012) (digital).cbz</title><guid>saga-cbz</guid>
      <enclosure url="https://idx/nzb/saga-cbz?apikey=SECRET"/>
      <pubDate>Wed, 01 Jan 2020 00:00:00 +0000</pubDate>
      <newznab:attr name="size" value="45234567"/>
    </item>
    <item>
      <title>Saga 012 (2012).cbr</title><guid>saga-cbr</guid>
      <enclosure url="https://idx/nzb/saga-cbr"/>
      <pubDate>Wed, 01 Jan 2020 00:00:00 +0000</pubDate>
      <newznab:attr name="size" value="40000000"/>
    </item>
    <item>
      <title>Batman 012 (2016) (digital).cbz</title><guid>batman</guid>
      <enclosure url="https://idx/nzb/batman"/>
      <pubDate>Wed, 01 Jan 2020 00:00:00 +0000</pubDate>
      <newznab:attr name="size" value="50000000"/>
    </item>
  </channel>
</rss>"#;

async fn seed_series_issue(db: &longbox_db::Pool) -> (i64, i64) {
    let series = series_repo::insert(
        db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Saga".to_owned(),
            sort_title: "saga".to_owned(),
            start_year: Some(2012),
            publisher: Some("Image".to_owned()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let issue = issue_repo::insert(
        db,
        NewIssue {
            series_id: series,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "12".to_owned(),
            title: None,
            cover_date: Some("2012-03-14".to_owned()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    (series, issue)
}

async fn seed_indexer(db: &longbox_db::Pool, name: &str, base_url: &str) {
    indexer_config_repo::insert(
        db,
        NewIndexerConfig {
            name: name.to_owned(),
            base_url: base_url.to_owned(),
            api_key: "KEY".to_owned(),
            enabled: true,
            priority: 0,
            maxage_days: 1500,
        },
    )
    .await
    .unwrap();
}

async fn newznab_serving(rss: &'static str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss))
        .mount(&server)
        .await;
    server
}

async fn search(app: &TestApp, body: serde_json::Value) -> serde_json::Value {
    let resp = app
        .request(json_request(
            "POST",
            "/api/search/interactive",
            body.to_string(),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    response_json(resp).await
}

#[tokio::test]
async fn returns_all_results_scored_and_sorted() {
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    let idx = newznab_serving(RSS).await;
    seed_indexer(&app.state.db, "NZBGeek", &idx.uri()).await;

    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;
    let results = body["results"].as_array().unwrap();

    // All three returned (no confidence threshold — user decides).
    assert_eq!(results.len(), 3);
    // Default sort: confidence descending — the two Saga releases first.
    let conf: Vec<f64> = results
        .iter()
        .map(|r| r["confidence"].as_f64().unwrap())
        .collect();
    assert!(
        conf[0] >= conf[1] && conf[1] >= conf[2],
        "not sorted desc: {conf:?}"
    );
    assert!(conf[0] > 0.9, "top result should be a strong Saga match");
    // The wrong-series Batman release is present but low-confidence.
    let batman = results
        .iter()
        .find(|r| r["title"].as_str().unwrap().contains("Batman"))
        .unwrap();
    assert!(batman["confidence"].as_f64().unwrap() < 0.5);

    // Per-result fields populated.
    let top = &results[0];
    assert_eq!(top["indexer"], "NZBGeek");
    assert!(top["size_bytes"].as_i64().unwrap() > 0);
    assert!(top["age_days"].as_i64().unwrap() > 0);
    assert!(top["nzb_url"].as_str().unwrap().starts_with("https://idx/"));

    assert!(body["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn partial_results_with_inline_indexer_error() {
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    let idx = newznab_serving(RSS).await;
    seed_indexer(&app.state.db, "Working", &idx.uri()).await;
    // An unreachable indexer must surface as an inline error, not fail
    // the whole search.
    seed_indexer(&app.state.db, "Dead", "http://127.0.0.1:1").await;

    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;
    assert_eq!(body["results"].as_array().unwrap().len(), 3);
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["indexer"], "Dead");
    let err_text = errors[0]["error"].as_str().unwrap();
    assert!(!err_text.is_empty());
    // The indexer API key must not leak into the error returned to the client.
    assert!(
        !err_text.contains("apikey"),
        "apikey leaked in error: {err_text}"
    );
    assert!(
        !err_text.contains("KEY"),
        "apikey leaked in error: {err_text}"
    );
}

#[tokio::test]
async fn empty_results_when_no_indexers() {
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;
    assert!(body["results"].as_array().unwrap().is_empty());
    assert!(body["errors"].as_array().unwrap().is_empty());
}

/// Give `issue_id` an owned, present file — what makes the catalog consider
/// the issue held. `is_present` matters as much as `status`: a row for bytes
/// that are gone from disk is not ownership.
async fn seed_owned_file(db: &longbox_db::Pool, issue_id: i64, is_present: bool) {
    seed_file(db, issue_id, is_present, "owned").await
}

/// As above, but the caller picks the status — both halves of the
/// `status='owned' AND is_present=1` predicate need their own test.
async fn seed_file(db: &longbox_db::Pool, issue_id: i64, is_present: bool, status: &str) {
    let root = library_root_repo::insert(
        db,
        NewLibraryRoot {
            path: "/comics".to_owned(),
        },
    )
    .await
    .unwrap()
    .id;
    let now = time::OffsetDateTime::now_utc();
    let now_p = time::PrimitiveDateTime::new(now.date(), now.time());
    file_repo::insert(
        db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: root,
            path_relative: "Saga (2012)/Saga (2012) 012.cbz".to_owned(),
            size_bytes: 45_000_000,
            mtime: now_p,
            last_scanned_at: now_p,
            match_method: "phase_b".to_owned(),
            match_confidence: 1.0,
            status: status.to_owned(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present,
            last_seen_at: now_p,
            matched_at: Some(now_p),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn search_warns_when_the_issue_is_already_owned_without_filtering_results() {
    // The warning is advisory, not a filter. Interactive Search is the
    // escape hatch for when the engine got it wrong, and wanting a
    // replacement copy is a legitimate reason to be here — so every result
    // must still come back.
    let mock = newznab_serving(RSS).await;
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    seed_indexer(&app.state.db, "NZBGeek", &mock.uri()).await;
    seed_owned_file(&app.state.db, issue, true).await;

    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;

    assert_eq!(body["already_owned"], json!(true));
    assert!(
        !body["results"].as_array().unwrap().is_empty(),
        "the warning must not suppress results: {body}"
    );
}

#[tokio::test]
async fn search_does_not_warn_for_an_issue_the_library_lacks() {
    let mock = newznab_serving(RSS).await;
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    seed_indexer(&app.state.db, "NZBGeek", &mock.uri()).await;
    // No file row at all.

    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;

    assert_eq!(body["already_owned"], json!(false));
}

#[tokio::test]
async fn a_catalogued_file_whose_bytes_are_gone_is_not_ownership() {
    // The distinction the whole warning rests on. A row with is_present=0 is
    // the catalog remembering a file that is no longer on disk — precisely
    // the case where the user SHOULD be re-downloading, so warning them off
    // would be worse than saying nothing.
    let mock = newznab_serving(RSS).await;
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    seed_indexer(&app.state.db, "NZBGeek", &mock.uri()).await;
    seed_owned_file(&app.state.db, issue, false).await;

    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;

    assert_eq!(
        body["already_owned"],
        json!(false),
        "a missing file is not a held issue"
    );
}

#[tokio::test]
async fn a_needs_review_file_is_not_ownership() {
    // The other half of the predicate. A needs-review file DOES carry the
    // issue link — `classify_status` only returns NeedsReview when an issue
    // matched — so `is_present` alone would call this owned. It is not: the
    // catalog is not confident this file is this issue, which is exactly
    // when a user should still be allowed to search without being told they
    // already have it.
    let mock = newznab_serving(RSS).await;
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    seed_indexer(&app.state.db, "NZBGeek", &mock.uri()).await;
    seed_file(&app.state.db, issue, true, "needs_review").await;

    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;

    assert_eq!(
        body["already_owned"],
        json!(false),
        "a needs-review file is not a held issue"
    );
}

#[tokio::test]
async fn ownership_is_scoped_to_the_issue_asked_about() {
    // Pins the WHERE clause. Without it, owning any issue anywhere would
    // warn on every search.
    let mock = newznab_serving(RSS).await;
    let app = build_test_app().await;
    let (series, issue) = seed_series_issue(&app.state.db).await;
    seed_indexer(&app.state.db, "NZBGeek", &mock.uri()).await;

    let other_issue = issue_repo::insert(
        &app.state.db,
        NewIssue {
            series_id: series,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "13".to_owned(),
            title: None,
            cover_date: Some("2012-04-18".to_owned()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    // Only the OTHER issue is owned.
    seed_owned_file(&app.state.db, other_issue, true).await;

    let body = search(
        &app,
        json!({ "series_id": series, "issue_id": issue, "query": "Saga 12" }),
    )
    .await;

    assert_eq!(
        body["already_owned"],
        json!(false),
        "owning issue 13 must not warn on a search for issue 12"
    );
}

#[tokio::test]
async fn unknown_series_is_404() {
    let app = build_test_app().await;
    let resp = app
        .request(json_request(
            "POST",
            "/api/search/interactive",
            json!({ "series_id": 9999, "issue_id": 1, "query": "x" }).to_string(),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------- grab ----------

/// Mock SABnzbd: GET /api → the given addurl JSON.
async fn sab_serving(body: &'static str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    server
}

async fn seed_sab(db: &longbox_db::Pool, base_url: &str) {
    downloader_config_repo::upsert(
        db,
        NewDownloaderConfig {
            kind: "sab".to_owned(),
            base_url: base_url.to_owned(),
            username: None,
            secret: "APIKEY".to_owned(),
            category: "comics".to_owned(),
            enabled: true,
        },
    )
    .await
    .unwrap();
}

async fn grab(app: &TestApp, body: serde_json::Value) -> serde_json::Value {
    let resp = app
        .request(json_request(
            "POST",
            "/api/search/interactive/grab",
            body.to_string(),
        ))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    response_json(resp).await
}

#[tokio::test]
async fn grab_submits_to_downloader() {
    let app = build_test_app().await;
    let sab = sab_serving(r#"{"status":true,"nzo_ids":["SABnzbd_nzo_abc"]}"#).await;
    seed_sab(&app.state.db, &sab.uri()).await;

    let body = grab(
        &app,
        json!({ "nzb_url": "https://idx/nzb/x?apikey=SECRET", "title": "Saga 012" }),
    )
    .await;
    assert_eq!(body["success"], true);
    assert!(body.get("error").is_none());
}

#[tokio::test]
async fn grab_surfaces_downloader_error() {
    let app = build_test_app().await;
    let sab = sab_serving(r#"{"status":false,"error":"Damaged NZB"}"#).await;
    seed_sab(&app.state.db, &sab.uri()).await;

    let body = grab(
        &app,
        json!({ "nzb_url": "https://idx/nzb/x", "title": "Saga 012" }),
    )
    .await;
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().contains("Damaged NZB"));
}

#[tokio::test]
async fn grab_without_downloader_configured_fails_cleanly() {
    let app = build_test_app().await;
    let body = grab(
        &app,
        json!({ "nzb_url": "https://idx/nzb/x", "title": "Saga 012" }),
    )
    .await;
    assert_eq!(body["success"], false);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("downloader"));
}
