//! Phase A.8 Step 6 — pull sweep integration tests.
//!
//! Each test drives `longbox_pull::sweep` against an in-memory catalog
//! and `wiremock` stand-ins for a Newznab indexer and a SABnzbd
//! downloader.

use longbox_db::{
    downloader_config_repo, file_repo, indexer_config_repo, issue_repo, library_root_repo,
    pull_attempt_repo, pull_list_repo, series_repo, NewDownloaderConfig, NewFile, NewIndexerConfig,
    NewIssue, NewLibraryRoot, NewPullAttempt, NewPullEntry, NewSeries, Pool,
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
            // Aligned with start_year=2012 so the year-gate sees the
            // same year as the `(2012)`-tagged mock NZBs the
            // retry-fresh-pick test uses. cover_date is now the
            // year_hint source per engine.rs, so this is the input
            // that drives the year-gate, not start_year.
            cover_date: Some("2012-07-01".into()),
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

    let summary = sweep(&db, None).await.unwrap();
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

    let summary = sweep(&db, None).await.unwrap();
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

    let summary = sweep(&db, None).await.unwrap();
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
        <item><title>Saga 001 (2012) a.cbz</title><guid>guid-old</guid>
              <enclosure url="http://nzb.example/old.nzb"/></item>
        <item><title>Saga 001 (2012) b.cbz</title><guid>guid-new</guid>
              <enclosure url="http://nzb.example/new.nzb"/></item>
      </channel>
    </rss>"#;
    indexer_returns(&indexer, two.into()).await;
    sab_accepts(&downloader, "nzo-2").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = sweep(&db, None).await.unwrap();
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

    let summary = sweep(&db, None).await.unwrap();
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

    let summary = sweep(&db, None).await.unwrap();
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

    let summary = sweep(&db, None).await.unwrap();
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
    let summary = sweep(&db, None).await.unwrap();
    assert_eq!(summary, longbox_pull::SweepSummary::default());
}

// -------- on-demand single-issue search --------

#[tokio::test]
async fn sweep_single_issue_404s_when_series_is_missing() {
    let (db, _, _) = seed_catalog().await;
    let err = longbox_pull::sweep_single_issue(&db, 99_999, 1)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            longbox_pull::PullError::SeriesNotFound { series_id: 99_999 }
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn sweep_single_issue_404s_when_issue_belongs_to_different_series() {
    let (db, series_id, _) = seed_catalog().await;
    // A second series + an issue that belongs to IT — passing the
    // wrong series_id with the right issue_id must surface as a
    // mismatch, not silently search the wrong scope.
    let other_series = series_repo::insert(
        &db,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: "Other".into(),
            sort_title: "other".into(),
            start_year: Some(2020),
            publisher: None,
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id;
    let other_issue = issue_repo::insert(
        &db,
        NewIssue {
            series_id: other_series,
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
    let err = longbox_pull::sweep_single_issue(&db, series_id, other_issue)
        .await
        .unwrap_err();
    match err {
        longbox_pull::PullError::IssueSeriesMismatch {
            series_id: req_series,
            issue_id,
            actual_series_id,
        } => {
            assert_eq!(req_series, series_id);
            assert_eq!(issue_id, other_issue);
            assert_eq!(actual_series_id, other_series);
        }
        other => panic!("expected IssueSeriesMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn sweep_single_issue_silent_no_op_without_a_downloader() {
    let (db, series_id, issue_id) = seed_catalog().await;
    // No downloader, no indexers. Same gate as the all-series sweep —
    // returns a default summary with a log line.
    let summary = longbox_pull::sweep_single_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(summary, longbox_pull::SweepSummary::default());
    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert!(attempts.is_empty(), "no attempts recorded");
}

/// LOAD-BEARING: when the engine sees an in-flight (`pending` /
/// `submitted` / `grabbed`) attempt for this issue, it must skip
/// silently and record nothing new. `pull_attempts` has no UNIQUE
/// constraint on (series_id, issue_id) — multiple rows per pair are
/// expected for retry tracking — so a duplicate INSERT would NOT be
/// absorbed by the schema. The guard inside `sweep_single_issue` is
/// the only thing that prevents a second click on Search from
/// duplicating an in-flight download.
#[tokio::test]
async fn sweep_single_issue_skips_when_an_in_flight_attempt_exists() {
    let (db, series_id, issue_id) = seed_catalog().await;
    // Pre-seed a submitted attempt — the in-flight state any user
    // click on Search needs to respect.
    pull_attempt_repo::insert(
        &db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-already-in-flight".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-already-in-flight".into()),
        },
    )
    .await
    .unwrap();
    let baseline = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(baseline.len(), 1, "exactly one in-flight attempt to start");

    // Set up a real indexer + downloader so the engine WOULD be able
    // to submit a duplicate if the guard weren't there. The guard
    // must fire BEFORE the indexer call.
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-second-search")).await;
    sab_accepts(&downloader, "nzo-second-search").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = longbox_pull::sweep_single_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    // Engine reported it did nothing. No submit, no failure, no
    // counter advance.
    assert_eq!(summary, longbox_pull::SweepSummary::default());

    // The seeded attempt is still the ONLY row. No duplicate insert,
    // no second download handle handed to SAB.
    let after = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        1,
        "in-flight guard must prevent a second pull_attempt row; got {} rows",
        after.len()
    );
    assert_eq!(after[0].id, baseline[0].id, "the existing row is preserved");
}

#[tokio::test]
async fn sweep_single_issue_purges_stale_submitted_before_in_flight_guard() {
    // Regression for the load-bearing bug: 87 stale `submitted` rows
    // in production were blocking Search-missing clicks because
    // their poll never resolved. The fix purges any `submitted` row
    // for the target issue older than STALE_SUBMITTED_HOURS (6h)
    // BEFORE the in-flight guard, so the user's explicit retry
    // actually fires.
    let (db, series_id, issue_id) = seed_catalog().await;

    // Seed a `submitted` row, then backdate it to 24h ago — older
    // than the 6h stale threshold.
    let stale = pull_attempt_repo::insert(
        &db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-stale".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-stale-orphan".into()),
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE pull_attempts SET attempted_at = datetime('now', '-24 hours') WHERE id = ?",
    )
    .bind(stale.id)
    .execute(&db)
    .await
    .unwrap();

    // Wire a real indexer + downloader so the engine can actually
    // submit once the stale row is out of the way.
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-fresh-retry")).await;
    sab_accepts(&downloader, "nzo-fresh-retry").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = longbox_pull::sweep_single_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        summary.submitted, 1,
        "must submit the retry after purging stale"
    );

    // Stale row gone, fresh `submitted` row in its place.
    let after = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(after.len(), 1, "exactly one row after purge + retry");
    assert_ne!(after[0].id, stale.id, "stale row must have been deleted");
    assert_eq!(after[0].status, "submitted");
    assert_eq!(after[0].release_id.as_deref(), Some("guid-fresh-retry"));
}

#[tokio::test]
async fn sweep_single_issue_purges_stale_grabbed_when_unowned() {
    // Live-DB regression: Absolute Batman #15-19 sat at status='grabbed'
    // for 4 days with zero owned files. SAB had silently failed (or
    // Phase B never picked the file up) and the row blocked
    // `sweep_single_issue`'s in-flight guard forever. Fix: purge
    // grabbed rows older than STALE_GRABBED_HOURS (24h) for the issue
    // when no owned+present file backs them.
    let (db, series_id, issue_id) = seed_catalog().await;

    // Seed a `grabbed` row backdated to 48h ago — past the 24h
    // threshold. No owned file is inserted, so it's a phantom grab.
    let stale = pull_attempt_repo::insert(
        &db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-grabbed-but-never-imported".into()),
            status: "grabbed".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-vanished".into()),
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE pull_attempts SET attempted_at = datetime('now', '-48 hours') WHERE id = ?",
    )
    .bind(stale.id)
    .execute(&db)
    .await
    .unwrap();

    // Wire real services so the retry can actually submit.
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-fresh-after-purge")).await;
    sab_accepts(&downloader, "nzo-fresh-after-purge").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = longbox_pull::sweep_single_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        summary.submitted, 1,
        "must submit the retry after purging the stale grabbed row"
    );

    // Stale grabbed row gone, fresh `submitted` row in its place.
    let after = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(after.len(), 1, "exactly one row after purge + retry");
    assert_ne!(
        after[0].id, stale.id,
        "stale grabbed row must have been deleted"
    );
    assert_eq!(after[0].status, "submitted");
    assert_eq!(
        after[0].release_id.as_deref(),
        Some("guid-fresh-after-purge")
    );
}

#[tokio::test]
async fn sweep_single_issue_preserves_grabbed_audit_when_issue_is_owned() {
    // Counterpart to the stale-grabbed purge: a `grabbed` row that
    // DOES have a matching owned+present file is the audit trail of
    // a successful pull. Its release_id feeds the exclusion list on
    // any future re-pulls. The SQL's NOT EXISTS clause is what
    // protects it. (The skip-owned guard in sweep_single_issue
    // short-circuits before purge runs in this path; this test
    // proves the purge SQL itself is also safe by exercising the
    // dismiss / workspace paths via a direct call.)
    let (db, series_id, issue_id) = seed_catalog().await;

    // Seed a `grabbed` row backdated past the threshold.
    let preserved = pull_attempt_repo::insert(
        &db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-real-successful-pull".into()),
            status: "grabbed".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-real".into()),
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE pull_attempts SET attempted_at = datetime('now', '-72 hours') WHERE id = ?",
    )
    .bind(preserved.id)
    .execute(&db)
    .await
    .unwrap();

    // Catalog the issue as owned-and-present — this row is the
    // RESULT of the successful pull above.
    let now = time::OffsetDateTime::now_utc();
    let now_pdt = time::PrimitiveDateTime::new(now.date(), now.time());
    file_repo::insert(
        &db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: 1,
            path_relative: "Saga (2012)/Saga (2012) 001.cbz".into(),
            size_bytes: 4096,
            mtime: now_pdt,
            last_scanned_at: now_pdt,
            match_method: "phase_b".into(),
            match_confidence: 1.0,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now_pdt,
            matched_at: Some(now_pdt),
        },
    )
    .await
    .unwrap();

    // Call the per-issue purge directly — guard the SQL, not the
    // sweep-level short-circuit.
    let purged = pull_attempt_repo::purge_stale_grabbed_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        purged, 0,
        "owned issue's grabbed audit row must be preserved"
    );

    let after = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, preserved.id, "audit row must survive");
    assert_eq!(after[0].status, "grabbed");
}

#[tokio::test]
async fn sweep_single_issue_still_blocks_on_fresh_submitted_under_threshold() {
    // Counterpart to the stale-purge test: a `submitted` row that's
    // FRESH (under 6h) MUST still block. The poll loop may yet
    // resolve it; we'd double-submit if the guard didn't fire.
    let (db, series_id, issue_id) = seed_catalog().await;

    // Default attempted_at = now → well under the 6h stale threshold.
    pull_attempt_repo::insert(
        &db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some("guid-fresh".into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some("nzo-still-in-flight".into()),
        },
    )
    .await
    .unwrap();

    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-should-not-submit")).await;
    sab_accepts(&downloader, "nzo-should-not-submit").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = longbox_pull::sweep_single_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(summary, longbox_pull::SweepSummary::default());

    let after = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        1,
        "fresh submitted must still block; no duplicate row"
    );
    assert_eq!(after[0].release_id.as_deref(), Some("guid-fresh"));
}

#[tokio::test]
async fn sweep_single_issue_skips_when_issue_is_already_owned() {
    // Regression: the on-demand single-issue path bypasses the
    // scheduled sweep's `list_pull_candidates` filter (which excludes
    // owned issues SQL-side), so without an engine-level guard the
    // per-issue Search button would happily submit a duplicate NZB
    // for an issue the catalog already owns. The fix lives in
    // `engine::sweep_single_issue` before the in-flight guard and
    // before any indexer call.
    let (db, series_id, issue_id) = seed_catalog().await;

    // Catalog the issue as owned-and-present. library_root_id=1 is
    // the only root seed_catalog inserted.
    let now = time::OffsetDateTime::now_utc();
    let now_pdt = time::PrimitiveDateTime::new(now.date(), now.time());
    file_repo::insert(
        &db,
        NewFile {
            issue_id: Some(issue_id),
            library_root_id: 1,
            path_relative: "Saga (2012)/Saga (2012) 001.cbz".into(),
            size_bytes: 4096,
            mtime: now_pdt,
            last_scanned_at: now_pdt,
            match_method: "phase_b".into(),
            match_confidence: 1.0,
            status: "owned".into(),
            cached_comicinfo_xml: None,
            cached_at: None,
            is_present: true,
            last_seen_at: now_pdt,
            matched_at: Some(now_pdt),
        },
    )
    .await
    .unwrap();

    // Wire a real indexer + downloader so a bug where the guard fails
    // to short-circuit would submit. The guard must fire BEFORE the
    // indexer call.
    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-should-not-be-fetched")).await;
    sab_accepts(&downloader, "nzo-should-not-be-handed-off").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = longbox_pull::sweep_single_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(summary, longbox_pull::SweepSummary::default());

    // No pull_attempts row was inserted — the guard fired before the
    // engine wrote anything.
    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert!(
        attempts.is_empty(),
        "owned-files guard must short-circuit before any attempt insert; got {} rows",
        attempts.len()
    );
}

#[tokio::test]
#[allow(non_snake_case)]
async fn sweep_single_issue_works_when_series_is_NOT_on_pull_list() {
    // The headline requirement: a series can be in the catalog
    // without being subscribed, and Search-now on one of its issues
    // still works.
    let (db, series_id, issue_id) = seed_catalog().await;
    // Drop the pull-list subscription seeded by seed_catalog so this
    // mirrors the "found a gap in my collection" path off the series
    // detail page for an unsubscribed series.
    pull_list_repo::remove(&db, series_id).await.unwrap();
    assert!(
        pull_list_repo::get(&db, series_id).await.unwrap().is_none(),
        "series must not be on the pull list for this test"
    );

    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    indexer_returns(&indexer, rss_one("guid-unsubscribed")).await;
    sab_accepts(&downloader, "nzo-unsubscribed").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;

    let summary = longbox_pull::sweep_single_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(summary.submitted, 1, "unsubscribed series can still submit");

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].status, "submitted");
    assert_eq!(attempts[0].release_id.as_deref(), Some("guid-unsubscribed"));
}

// -------- Phase 1: the completed-but-empty feedback edge --------
//
// Before this existed, a download that arrived perfectly and contained
// no comic had no way to report itself: Phase B settles attempts by
// matching an issue, and there is nothing to match. The attempt sat
// `submitted` until SABnzbd forgot the job, then surfaced — three days
// later — as "lost track of download". Nothing was lost.

/// Scratch directory standing in for the watch root, removed on drop.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "longbox-sweep-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        Self(base)
    }
    /// Create `<root>/<job>/<file>`, mirroring how SAB deposits a job.
    fn job_file(&self, job: &str, file: &str) -> &Self {
        let dir = self.0.join(job);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), b"x").unwrap();
        self
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    /// Force an mtime so the folder guards can be driven end to end.
    fn age(&self, rel: &str, unix_secs: i64) -> &Self {
        let p = if rel.is_empty() {
            self.0.clone()
        } else {
            self.0.join(rel)
        };
        filetime::set_file_mtime(&p, filetime::FileTime::from_unix_time(unix_secs, 0))
            .expect("set mtime");
        self
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// SAB reports an empty queue and one Completed history slot whose
/// `storage` points at `job` under some absolute path. The path is
/// deliberately NOT the test's watch root — SAB reports its own
/// namespace and only the final component is meaningful.
async fn sab_completed_at(server: &MockServer, nzo_id: &str, job: &str) {
    // "Now", because these fixtures create their folders fresh and do
    // not age them — so the folder's mtime is now and this puts the
    // job inside the completion window. SAB reports `completed` on
    // every history slot, so a real timestamp is also the faithful
    // default; a fixture that omitted it would only exercise the
    // no-timestamp abstention.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64;
    sab_completed_at_time(server, nzo_id, job, now).await;
}

/// As above, but with SAB's own `completed` timestamp (Unix seconds).
/// `0` means "unreported", which is what the older fixtures used and
/// what leaves the completion-window guard switched off.
async fn sab_completed_at_time(server: &MockServer, nzo_id: &str, job: &str, completed: i64) {
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue": {"slots": []}}"#))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"{{"history": {{"slots": [
                {{"nzo_id": "{nzo_id}", "status": "Completed", "fail_message": "",
                  "storage": "/sab/complete/{job}", "completed": {completed}}}
            ]}}}}"#
        )))
        .mount(server)
        .await;
}

/// Seed one in-flight attempt carrying a handle and a release guid.
async fn seed_submitted(db: &Pool, series_id: i64, issue_id: i64, guid: &str, nzo_id: &str) {
    pull_attempt_repo::insert(
        db,
        NewPullAttempt {
            series_id,
            issue_id,
            indexer_id: None,
            release_id: Some(guid.into()),
            status: "submitted".into(),
            error_message: None,
            retry_count: 0,
            download_handle: Some(nzo_id.into()),
        },
    )
    .await
    .unwrap();
}

/// MECHANISM 1 — the settle-on-empty condition.
///
/// A completed download whose folder holds only an EPUB settles now,
/// with a message describing what actually happened.
#[tokio::test]
async fn a_completed_download_with_no_comics_settles_honestly() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("nocomics");
    // The `.1` is SAB's collision suffix — the folder that really
    // exists, and one no reconstruction from the job name produces.
    watch
        .job_file("Saga 1.1", "Saga 001.epub")
        .job_file("Saga 1.1", "info.nfo");

    let downloader = MockServer::start().await;
    sab_completed_at(&downloader, "nzo-ebook", "Saga 1.1").await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-ebook", "nzo-ebook").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(summary.completed_without_comics, 1);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].status, "failed",
        "a completed-but-empty download must settle, not linger as submitted"
    );
    let msg = attempts[0].error_message.clone().unwrap_or_default();
    assert!(
        msg.contains("no comic files"),
        "the recorded reason must say what happened, got {msg:?}"
    );
    assert!(
        !msg.contains("lost track"),
        "nothing was lost — the download arrived, got {msg:?}"
    );
}

/// MECHANISM 2 — the terminal state suppresses the release.
///
/// Settling must keep `release_id` on the row, because the exclusion
/// list for the next attempt is built from the guids of prior
/// attempts. Without that, the engine re-grabs the same release forever.
#[tokio::test]
async fn the_settled_release_is_not_grabbed_again_for_that_issue() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("suppress");
    watch.job_file("Saga 1", "Saga 001.epub");

    let indexer = MockServer::start().await;
    let downloader = MockServer::start().await;
    // The indexer offers exactly one release — the one that just
    // proved itself empty.
    indexer_returns(&indexer, rss_one("guid-ebook")).await;
    sab_completed_at(&downloader, "nzo-ebook", "Saga 1").await;
    sab_accepts(&downloader, "nzo-second").await;
    add_indexer(&db, indexer.uri()).await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-ebook", "nzo-ebook").await;

    // One sweep: Phase 1 settles the attempt, then Phase 2 re-searches
    // the now-eligible issue and must refuse the same release.
    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(
        summary.submitted, 0,
        "the release that contained no comics must not be grabbed again"
    );

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        attempts[0].release_id.as_deref(),
        Some("guid-ebook"),
        "settling must preserve the guid — it is what drives the exclusion"
    );
    assert!(
        attempts[0].retry_count >= 1,
        "the settle must count against the retry budget so the issue eventually parks"
    );
}

/// The guard against the failure mode that matters: a wrong condition
/// writes a terminal state that starves an issue forever, and it looks
/// exactly like the feature working.
#[tokio::test]
async fn a_completed_download_containing_a_comic_is_left_alone() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("hascomic");
    watch.job_file("Saga 1", "Saga 001.cbz");

    let downloader = MockServer::start().await;
    sab_completed_at(&downloader, "nzo-ok", "Saga 1").await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-ok", "nzo-ok").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(summary.completed_without_comics, 0);
    // `HasComicFiles` is the one verdict with no counter of its own,
    // so without these two this test passes when the check never ran
    // at all — the only assertion it could make that proves the check
    // reached the folder and declined on the merits.
    assert_eq!(summary.landed_abstained, 0);
    assert_eq!(summary.landed_unresolved, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        attempts[0].status, "submitted",
        "Phase B owns this one — the pull engine must not touch it"
    );
}

/// A folder the user cleared by hand is not evidence of anything. This
/// is the limitation the PR states: those attempts stay in flight and
/// still age out as "lost track of download".
#[tokio::test]
async fn a_missing_folder_leaves_the_attempt_in_flight() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("cleared");

    let downloader = MockServer::start().await;
    sab_completed_at(&downloader, "nzo-gone", "Saga 1").await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-gone", "nzo-gone").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(summary.completed_without_comics, 0);
    assert_eq!(
        summary.landed_abstained, 1,
        "the check must be reached and decline — not silently skipped"
    );
    assert_eq!(summary.landed_unresolved, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "submitted");
}

/// Pins the SECOND `landed_unresolved` increment — the
/// `basename() == None` branch. Mutating it alone previously failed
/// zero tests; the other pinned test only covers the first.
#[tokio::test]
async fn a_location_with_no_final_component_is_counted_as_unresolved() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("nobase");

    let downloader = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue": {"slots": []}}"#))
        .mount(&downloader)
        .await;
    // A reported location with no usable final component.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"history": {"slots": [
                {"nzo_id": "nzo-nobase", "status": "Completed", "fail_message": "",
                 "storage": "/", "completed": 1780000000}
            ]}}"#,
        ))
        .mount(&downloader)
        .await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-nobase", "nzo-nobase").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(summary.landed_unresolved, 1);
    assert_eq!(summary.landed_abstained, 0);
    assert_eq!(summary.completed_without_comics, 0);
}

/// With no watch root configured (Phase B disabled) the engine behaves
/// exactly as it did before this change.
#[tokio::test]
async fn without_a_watch_root_the_check_does_not_run() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let downloader = MockServer::start().await;
    sab_completed_at(&downloader, "nzo-x", "Saga 1").await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-x", "nzo-x").await;

    let summary = sweep(&db, None).await.unwrap();
    assert_eq!(summary.completed_without_comics, 0);
    // Phase B disabled is a permanent config state, so it must NOT
    // pollute `landed_unresolved` — that counter has to keep meaning
    // "something went wrong today".
    assert_eq!(summary.landed_unresolved, 0);
    assert_eq!(summary.landed_abstained, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "submitted");
}

// -------- the two folder guards, exercised through the real sweep --------
//
// The unit tests in `landed.rs` prove each guard's logic. These prove
// the *wiring* reaches them: `settle_if_nothing_landed` joins SAB's
// `storage` basename onto the watch root and forwards SAB's
// `completed`, and those two inputs are the whole subject of the
// revision. A guard that is never invoked is indistinguishable from a
// guard that was deleted.

/// A plausible completion time, matching the unit tests' fixture clock.
const JOB_COMPLETED: i64 = 1_780_000_000;

#[tokio::test]
async fn guard_one_end_to_end_a_stale_attempt_is_not_judged_against_a_newer_folder() {
    // SAB says the job completed a day before the folder now sitting
    // at that path was written — so the name was re-allocated and this
    // folder belongs to someone else.
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("g1");
    watch.job_file("Saga 1", "info.nfo");
    watch
        .age("Saga 1/info.nfo", JOB_COMPLETED + 86_400)
        .age("Saga 1", JOB_COMPLETED + 86_400);

    let downloader = MockServer::start().await;
    sab_completed_at_time(&downloader, "nzo-stale", "Saga 1", JOB_COMPLETED).await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-stale", "nzo-stale").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(
        summary.completed_without_comics, 0,
        "a folder written a day after the job completed must not be judged"
    );
    assert_eq!(
        summary.landed_abstained, 1,
        "the guard must be reached — a silent no-op passes a condemn-count assertion too"
    );
    assert_eq!(
        summary.landed_unresolved, 0,
        "abstaining because no folder could be resolved is not the same as the guard declining"
    );

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "submitted");
}

#[tokio::test]
async fn guard_two_end_to_end_a_folder_phase_b_emptied_is_not_condemned() {
    // The F1 shape, nested: Phase B took the comic out of `disc1` and
    // left the release's litter. Only the subdirectory's mtime moved.
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("g2");
    // A file directly in the job root as well, so the root is
    // evaluable and this fixture exercises the REMOVAL path rather
    // than abstaining via `DirectoryNotEvaluable` — otherwise it
    // passes for the wrong reason and stops distinguishing guard 2.
    watch
        .job_file("Saga 1", "info.nfo")
        .job_file("Saga 1/disc1", "info.nfo");
    watch
        .age("Saga 1/info.nfo", JOB_COMPLETED)
        .age("Saga 1/disc1/info.nfo", JOB_COMPLETED)
        .age("Saga 1/disc1", JOB_COMPLETED + 600)
        .age("Saga 1", JOB_COMPLETED);

    let downloader = MockServer::start().await;
    sab_completed_at_time(&downloader, "nzo-emptied", "Saga 1", JOB_COMPLETED).await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-emptied", "nzo-emptied").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(
        summary.completed_without_comics, 0,
        "a folder something was removed from must not be condemned"
    );
    assert_eq!(
        summary.landed_abstained, 1,
        "the guard must be reached — a silent no-op passes a condemn-count assertion too"
    );
    assert_eq!(
        summary.landed_unresolved, 0,
        "abstaining because no folder could be resolved is not the same as the guard declining"
    );

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "submitted");
}

#[tokio::test]
async fn both_guards_satisfied_still_condemns_a_real_ebook_release() {
    // Negative control for the pair. If either guard were over-eager
    // the feature would abstain on everything and the two tests above
    // would still pass.
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("g3");
    watch
        .job_file("Saga 1", "release.epub")
        .job_file("Saga 1", "info.nfo");
    watch
        .age("Saga 1/release.epub", JOB_COMPLETED)
        .age("Saga 1/info.nfo", JOB_COMPLETED)
        .age("Saga 1", JOB_COMPLETED);

    let downloader = MockServer::start().await;
    sab_completed_at_time(&downloader, "nzo-ebook2", "Saga 1", JOB_COMPLETED).await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-ebook2", "nzo-ebook2").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(summary.completed_without_comics, 1);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "failed");
}

/// The per-directory comparison, end to end. Under the previous
/// global `max(dirs)` vs `max(files)` test, `disc2`'s untouched file
/// is as new as `disc1`'s removal bump, the delta collapses to zero,
/// and this folder is condemned. Only a per-directory comparison
/// abstains here — so this test fails if that is reverted.
#[tokio::test]
async fn per_directory_comparison_end_to_end_a_sibling_cannot_mask_a_removal() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("perdir");
    watch
        .job_file("Saga 1", "info.nfo")
        .job_file("Saga 1/disc1", "info.nfo")
        .job_file("Saga 1/disc2", "extra.nfo");
    watch
        .age("Saga 1/info.nfo", JOB_COMPLETED)
        .age("Saga 1/disc1/info.nfo", JOB_COMPLETED)
        .age("Saga 1/disc1", JOB_COMPLETED + 600)
        .age("Saga 1/disc2/extra.nfo", JOB_COMPLETED + 600)
        .age("Saga 1/disc2", JOB_COMPLETED + 600)
        .age("Saga 1", JOB_COMPLETED);

    let downloader = MockServer::start().await;
    sab_completed_at_time(&downloader, "nzo-perdir", "Saga 1", JOB_COMPLETED).await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-perdir", "nzo-perdir").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(
        summary.completed_without_comics, 0,
        "a file in a sibling directory must not vouch for disc1"
    );
    assert_eq!(
        summary.landed_abstained, 1,
        "the abstention must be counted, or it is invisible in production"
    );
    assert_eq!(summary.landed_unresolved, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "submitted");
}

/// Pins `landed_unresolved`. Without this, deleting both increments in
/// `settle_if_nothing_landed` fails **zero** tests — every other
/// assertion on this counter is `== 0`.
///
/// That matters more than an ordinary coverage gap: this counter is
/// the designated reveal mechanism for "the feature is dead because
/// the downloader's location field is wrong", and its only live
/// trigger is NZBGet, whose field names have never been checked
/// against a running server. An unpinned reveal mechanism reveals
/// nothing.
#[tokio::test]
async fn a_download_with_no_reported_location_is_counted_as_unresolved() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("noloc");

    let downloader = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "queue"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"queue": {"slots": []}}"#))
        .mount(&downloader)
        .await;
    // Completed, with no `storage` at all — the shape a wrong field
    // name produces on every download, forever.
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("mode", "history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"history": {"slots": [
                {"nzo_id": "nzo-noloc", "status": "Completed", "fail_message": "",
                 "completed": 1780000000}
            ]}}"#,
        ))
        .mount(&downloader)
        .await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-noloc", "nzo-noloc").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(
        summary.landed_unresolved, 1,
        "a completed download with no location must be visible, not silent"
    );
    assert_eq!(
        summary.landed_abstained, 0,
        "the check never ran; it did not decline"
    );
    assert_eq!(summary.completed_without_comics, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "submitted");
}

/// The inverted condition, end to end.
///
/// The unit tests prove `classify`'s logic; this proves the wiring
/// reaches it — `storage.basename()` joined onto the watch root, the
/// verdict carried back into a counter and a DB decision. Four unit
/// tests failed under mutation of this condition and **zero** sweep
/// tests did, on the newest code in the branch, in the condemn
/// direction.
///
/// The folder holds an unextracted container. It appears on no list,
/// which is the point: it abstains because it is UNRECOGNISED, not
/// because anyone enumerated `.rar`.
#[tokio::test]
async fn unrecognised_content_end_to_end_is_not_condemned() {
    let (db, series_id, issue_id) = seed_catalog().await;
    let watch = Scratch::new("unrec-e2e");
    watch
        .job_file("Saga 1", "Saga 001.part01.rar")
        .job_file("Saga 1", "info.nfo");
    watch
        .age("Saga 1/Saga 001.part01.rar", JOB_COMPLETED)
        .age("Saga 1/info.nfo", JOB_COMPLETED)
        .age("Saga 1", JOB_COMPLETED);

    let downloader = MockServer::start().await;
    sab_completed_at_time(&downloader, "nzo-unrec", "Saga 1", JOB_COMPLETED).await;
    add_sab_downloader(&db, downloader.uri()).await;
    seed_submitted(&db, series_id, issue_id, "guid-unrec", "nzo-unrec").await;

    let summary = sweep(&db, Some(watch.path())).await.unwrap();
    assert_eq!(
        summary.completed_without_comics, 0,
        "an unrecognised file means we cannot say the comic is absent"
    );
    assert_eq!(
        summary.landed_abstained, 1,
        "the check must be reached and decline — a silent no-op also condemns nothing"
    );
    assert_eq!(summary.landed_unresolved, 0);

    let attempts = pull_attempt_repo::list_for_issue(&db, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(
        attempts[0].status, "submitted",
        "the release must not be suppressed — the comic may be inside the rar"
    );
}
