//! Phase A.8 Step 6 — pull sweep integration tests.
//!
//! Each test drives `longbox_pull::sweep` against an in-memory catalog
//! and `wiremock` stand-ins for a Newznab indexer and a SABnzbd
//! downloader.

use longbox_db::{
    downloader_config_repo, file_repo, indexer_config_repo, issue_repo, library_root_repo,
    pull_attempt_repo, pull_list_repo, series_repo, NewDownloaderConfig, NewFile,
    NewIndexerConfig, NewIssue, NewLibraryRoot, NewPullAttempt, NewPullEntry, NewSeries, Pool,
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

// -------- on-demand single-issue search --------

#[tokio::test]
async fn sweep_single_issue_404s_when_series_is_missing() {
    let (db, _, _) = seed_catalog().await;
    let err = longbox_pull::sweep_single_issue(&db, 99_999, 1)
        .await
        .unwrap_err();
    assert!(
        matches!(err, longbox_pull::PullError::SeriesNotFound { series_id: 99_999 }),
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
    sqlx::query("UPDATE pull_attempts SET attempted_at = datetime('now', '-24 hours') WHERE id = ?")
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
    assert_eq!(summary.submitted, 1, "must submit the retry after purging stale");

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
    sqlx::query("UPDATE pull_attempts SET attempted_at = datetime('now', '-48 hours') WHERE id = ?")
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
    assert_ne!(after[0].id, stale.id, "stale grabbed row must have been deleted");
    assert_eq!(after[0].status, "submitted");
    assert_eq!(after[0].release_id.as_deref(), Some("guid-fresh-after-purge"));
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
    sqlx::query("UPDATE pull_attempts SET attempted_at = datetime('now', '-72 hours') WHERE id = ?")
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
    assert_eq!(purged, 0, "owned issue's grabbed audit row must be preserved");

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
    assert_eq!(after.len(), 1, "fresh submitted must still block; no duplicate row");
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
