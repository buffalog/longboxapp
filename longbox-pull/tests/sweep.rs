//! Phase A.8 Step 6 — pull sweep integration tests.
//!
//! Each test drives `longbox_pull::sweep` against an in-memory catalog
//! and `wiremock` stand-ins for a Newznab indexer and a SABnzbd
//! downloader.

use longbox_db::{
    downloader_config_repo, indexer_config_repo, issue_repo, library_root_repo, pull_attempt_repo,
    pull_list_repo, series_repo, NewDownloaderConfig, NewIndexerConfig, NewIssue, NewLibraryRoot,
    NewPullAttempt, NewPullEntry, NewSeries, Pool,
};
use longbox_pull::sweep;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// In-memory catalog with one pulled series ("Saga") and one shipped,
/// un-owned issue (#1, cover-dated well in the past). Returns the pool
/// and the (series_id, issue_id).
async fn seed_catalog() -> (Pool, i64, i64) {
    let db = longbox_db::open(":memory:").await.unwrap();
    library_root_repo::insert(
        &db,
        NewLibraryRoot {
            path: "/tmp/longbox-pull-test".into(),
        },
    )
    .await
    .unwrap();
    let series_id = series_repo::insert(
        &db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Saga".into(),
            sort_title: "saga".into(),
            start_year: Some(2012),
            publisher: Some("Image".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let issue_id = issue_repo::insert(
        &db,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: "1".into(),
            title: None,
            cover_date: Some("2024-01-01".into()),
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    pull_list_repo::add(
        &db,
        NewPullEntry {
            series_id,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    (db, series_id, issue_id)
}

async fn add_indexer(db: &Pool, base_url: String) {
    indexer_config_repo::insert(
        db,
        NewIndexerConfig {
            name: "test-indexer".into(),
            base_url,
            api_key: "KEY".into(),
            enabled: true,
            priority: 0,
            maxage_days: 1500,
        },
    )
    .await
    .unwrap();
}

async fn add_sab_downloader(db: &Pool, base_url: String) {
    downloader_config_repo::upsert(
        db,
        NewDownloaderConfig {
            kind: "sab".into(),
            base_url,
            username: None,
            secret: "KEY".into(),
            category: String::new(),
            enabled: true,
        },
    )
    .await
    .unwrap();
}

/// A one-item Newznab RSS for the given guid.
fn rss_one(guid: &str) -> String {
    format!(
        r#"<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
          <channel>
            <item><title>Saga 001.cbz</title><guid>{guid}</guid>
                  <enclosure url="http://nzb.example/{guid}.nzb"/></item>
          </channel>
        </rss>"#
    )
}

/// Mount a Newznab `t=search` response on the indexer mock.
async fn indexer_returns(server: &MockServer, body: String) {
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("t", "search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

/// Mount a SABnzbd `mode=addurl` response that accepts the NZB.
async fn sab_accepts(server: &MockServer, nzo_id: &str) {
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "addurl"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"status":true,"nzo_ids":["{nzo_id}"]}}"#)),
        )
        .mount(server)
        .await;
}

// -------- Phase 2: submitting new pulls --------

#[tokio::test]
async fn submits_a_matched_release_and_records_the_attempt() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-saga-1")).await;
    sab_accepts(&downloader, "nzo-1").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db).await.unwrap();
    assert_eq!(summary.submitted, 1);
    assert_eq!(summary.no_match, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].status, "submitted");
    assert_eq!(attempts[0].release_id.as_deref(), Some("guid-saga-1"));
    assert_eq!(attempts[0].download_handle.as_deref(), Some("nzo-1"));
}

#[tokio::test]
async fn no_indexer_match_records_no_attempt() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    // Well-formed but empty — a clean zero-result search.
    indexer_returns(&indexer, "<rss><channel></channel></rss>".into()).await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db).await.unwrap();
    assert_eq!(summary.no_match, 1);
    assert_eq!(summary.submitted, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert!(attempts.is_empty(), "no-match must not record an attempt");
}

#[tokio::test]
async fn submission_failure_records_a_failed_attempt_with_retry_count() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-saga-1")).await;
    // SAB rejects the NZB.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "addurl"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"status":false,"error":"rejected"}"#),
        )
        .mount(&downloader)
        .await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db).await.unwrap();
    assert_eq!(summary.submission_failed, 1);
    assert_eq!(summary.submitted, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[0].retry_count, 1);
    // A submission failure records no release_id — the same release is
    // retried next sweep.
    assert_eq!(attempts[0].release_id, None);
}

#[tokio::test]
async fn grab_retry_excludes_an_already_tried_release() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;

    // A prior grab-failed attempt for guid-old (a failed attempt that
    // carries a release_id — distinct from a submission failure).
    pull_attempt_repo::insert(
        &db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-old".into()),
            status: "failed".into(),
            error_message: Some("download failed".into()),
            retry_count: 1,
            download_handle: None,
        },
    )
    .await
    .unwrap();

    // The indexer offers both the tried release and a fresh one.
    let two = r#"<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
      <channel>
        <item><title>Saga 001 a.cbz</title><guid>guid-old</guid>
              <enclosure url="http://nzb.example/old.nzb"/></item>
        <item><title>Saga 001 b.cbz</title><guid>guid-new</guid>
              <enclosure url="http://nzb.example/new.nzb"/></item>
      </channel>
    </rss>"#;
    indexer_returns(&indexer, two.into()).await;
    sab_accepts(&downloader, "nzo-2").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db).await.unwrap();
    assert_eq!(summary.submitted, 1);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    // The fresh submitted attempt picked guid-new, not the tried guid-old.
    let submitted = attempts
        .iter()
        .find(|a| a.status == "submitted")
        .expect("a submitted attempt");
    assert_eq!(submitted.release_id.as_deref(), Some("guid-new"));
    // retry_count carries the one prior failure forward.
    assert_eq!(submitted.retry_count, 1);
}

#[tokio::test]
async fn parked_issue_at_the_retry_ceiling_is_not_re_attempted() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    // An attempt already at the parking threshold.
    pull_attempt_repo::insert(
        &db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: None,
            status: "failed".into(),
            error_message: Some("gave up".into()),
            retry_count: 3,
            download_handle: None,
        },
    )
    .await
    .unwrap();
    indexer_returns(&indexer, rss_one("guid-saga-1")).await;
    sab_accepts(&downloader, "nzo-x").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db).await.unwrap();
    // The parked issue is not a candidate — nothing submitted.
    assert_eq!(summary.submitted, 0);
    assert_eq!(summary.no_match, 0);
}

// -------- Phase 1: polling in-flight attempts --------

/// Seed one `submitted` attempt with a download handle, no indexers
/// configured (so the sweep is poll-only).
async fn seed_in_flight(db: &Pool, series_id: i64, issue_id: i64, handle: &str) -> i64 {
    pull_attempt_repo::insert(
        db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-x".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some(handle.into()),
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn polling_a_failed_download_fails_the_attempt() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let downloader = MockServer::start().await;
    let attempt_id = seed_in_flight(&db, series_id, issue_id, "nzo-1").await;

    // SAB: not in the queue, present in history as Failed.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue":{"slots":[]}}"#))
        .mount(&downloader)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"history":{"slots":[{"nzo_id":"nzo-1","status":"Failed","fail_message":"par2"}]}}"#,
        ))
        .mount(&downloader)
        .await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db).await.unwrap();
    assert_eq!(summary.polled, 1);
    assert_eq!(summary.grab_failed, 1);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    let polled = attempts.iter().find(|a| a.id == attempt_id).unwrap();
    assert_eq!(polled.status, "failed");
    assert_eq!(polled.retry_count, 1);
}

#[tokio::test]
async fn polling_an_unknown_download_bumps_the_counter_below_the_limit() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let downloader = MockServer::start().await;
    let attempt_id = seed_in_flight(&db, series_id, issue_id, "nzo-1").await;

    // SAB: not in the queue, not in history — Unknown.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue":{"slots":[]}}"#))
        .mount(&downloader)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"history":{"slots":[]}}"#))
        .mount(&downloader)
        .await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db).await.unwrap();
    assert_eq!(summary.polled, 1);
    // One Unknown is below the give-up threshold — still in flight.
    assert_eq!(summary.grab_failed, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    let polled = attempts.iter().find(|a| a.id == attempt_id).unwrap();
    assert_eq!(polled.status, "submitted");
    assert_eq!(polled.unknown_polls, 1);
}

#[tokio::test]
async fn sweep_without_a_downloader_is_a_no_op() {
    let (db, _series_id, _issue_id) = seed_catalog().await;
    // No downloader, no indexers configured.
    let summary = sweep(&db).await.unwrap();
    assert_eq!(summary, longbox_pull::SweepSummary::default());
}
