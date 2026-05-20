//! Repository tests for the Phase A.8 Step 3 tables: downloader /
//! indexer / webhook config, pull_list, pull_attempts, release cache.

mod common;

use common::fresh_pool;
use longbox_db::webhook_config_repo::{
    EVENT_NEW_SOLICITATIONS, EVENT_PULL_FAILED, EVENT_PULL_SUCCEEDED,
};
use longbox_db::{
    downloader_config_repo, indexer_config_repo, issue_repo, pull_attempt_repo, pull_list_repo,
    release_cache_repo, series_repo, webhook_config_repo, DbError, NewDownloaderConfig,
    NewIndexerConfig, NewIssue, NewPullAttempt, NewPullEntry, NewReleaseCacheEntry, NewSeries,
    NewWebhookConfig,
};
use sqlx::SqlitePool;
use time::macros::datetime;

async fn seed_series(pool: &SqlitePool, title: &str) -> i64 {
    series_repo::insert(
        pool,
        NewSeries {
            cv_id: None,
            metron_id: None,
            title: title.into(),
            sort_title: title.to_lowercase(),
            start_year: Some(2024),
            publisher: Some("Image".into()),
            description: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

async fn seed_issue(pool: &SqlitePool, series_id: i64, number: &str) -> i64 {
    issue_repo::insert(
        pool,
        NewIssue {
            series_id,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.into(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap()
    .id
}

// -------- downloader_config (single-row) --------

#[tokio::test]
async fn downloader_config_get_empty_is_none() {
    let pool = fresh_pool().await;
    assert!(downloader_config_repo::get(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn downloader_config_upsert_then_get() {
    let pool = fresh_pool().await;
    downloader_config_repo::upsert(
        &pool,
        NewDownloaderConfig {
            kind: "sab".into(),
            base_url: "http://localhost:8080".into(),
            username: None,
            secret: "APIKEY".into(),
            category: "comics".into(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    let row = downloader_config_repo::get(&pool).await.unwrap().unwrap();
    assert_eq!(row.id, 1);
    assert_eq!(row.kind, "sab");
    assert_eq!(row.secret, "APIKEY");
    assert!(row.username.is_none());
    assert!(row.enabled);
}

#[tokio::test]
async fn downloader_config_upsert_replaces_the_single_row() {
    let pool = fresh_pool().await;
    downloader_config_repo::upsert(
        &pool,
        NewDownloaderConfig {
            kind: "sab".into(),
            base_url: "http://old".into(),
            username: None,
            secret: "k1".into(),
            category: String::new(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    // Switch to an NZBGet config — Basic auth, so username is set.
    downloader_config_repo::upsert(
        &pool,
        NewDownloaderConfig {
            kind: "nzbget".into(),
            base_url: "http://new:6789".into(),
            username: Some("nzbget".into()),
            secret: "tegbzn".into(),
            category: "comics".into(),
            enabled: false,
        },
    )
    .await
    .unwrap();
    let row = downloader_config_repo::get(&pool).await.unwrap().unwrap();
    assert_eq!(row.id, 1, "still exactly one row, id=1");
    assert_eq!(row.kind, "nzbget");
    assert_eq!(row.username.as_deref(), Some("nzbget"));
    assert!(!row.enabled);
}

#[tokio::test]
async fn downloader_config_clear() {
    let pool = fresh_pool().await;
    downloader_config_repo::upsert(
        &pool,
        NewDownloaderConfig {
            kind: "sab".into(),
            base_url: "http://x".into(),
            username: None,
            secret: "k".into(),
            category: String::new(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    downloader_config_repo::clear(&pool).await.unwrap();
    assert!(downloader_config_repo::get(&pool).await.unwrap().is_none());
}

// -------- indexer_configs --------

fn new_indexer(name: &str, priority: i64, enabled: bool) -> NewIndexerConfig {
    NewIndexerConfig {
        name: name.into(),
        base_url: format!("https://{name}.example.com"),
        api_key: "KEY".into(),
        enabled,
        priority,
        maxage_days: 1500,
    }
}

#[tokio::test]
async fn indexer_list_enabled_filters_and_orders_by_priority() {
    let pool = fresh_pool().await;
    indexer_config_repo::insert(&pool, new_indexer("slow", 9, true))
        .await
        .unwrap();
    indexer_config_repo::insert(&pool, new_indexer("fast", 0, true))
        .await
        .unwrap();
    indexer_config_repo::insert(&pool, new_indexer("off", 5, false))
        .await
        .unwrap();

    let enabled = indexer_config_repo::list_enabled(&pool).await.unwrap();
    let names: Vec<&str> = enabled.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["fast", "slow"],
        "priority order, disabled excluded"
    );

    assert_eq!(indexer_config_repo::list_all(&pool).await.unwrap().len(), 3);
}

#[tokio::test]
async fn indexer_update_and_delete() {
    let pool = fresh_pool().await;
    let row = indexer_config_repo::insert(&pool, new_indexer("a", 0, true))
        .await
        .unwrap();
    indexer_config_repo::update(
        &pool,
        row.id,
        indexer_config_repo::IndexerConfigUpdate {
            name: "a-renamed".into(),
            base_url: row.base_url.clone(),
            api_key: row.api_key.clone(),
            enabled: false,
            priority: 3,
            maxage_days: 90,
        },
    )
    .await
    .unwrap();
    let got = indexer_config_repo::get(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.name, "a-renamed");
    assert_eq!(got.maxage_days, 90);
    assert!(!got.enabled);

    indexer_config_repo::delete(&pool, row.id).await.unwrap();
    assert!(indexer_config_repo::get(&pool, row.id)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        indexer_config_repo::delete(&pool, row.id).await,
        Err(DbError::NotFound)
    ));
}

// -------- webhook_configs --------

#[tokio::test]
async fn webhook_list_subscribed_matches_the_event_bit() {
    let pool = fresh_pool().await;
    // Subscribes to succeeded + failed, not solicitations.
    webhook_config_repo::insert(
        &pool,
        NewWebhookConfig {
            name: "ops".into(),
            url: "https://hooks.slack.com/x".into(),
            event_mask: EVENT_PULL_SUCCEEDED | EVENT_PULL_FAILED,
            enabled: true,
        },
    )
    .await
    .unwrap();
    // Subscribes to solicitations only, but disabled.
    webhook_config_repo::insert(
        &pool,
        NewWebhookConfig {
            name: "discovery".into(),
            url: "https://example.com/h".into(),
            event_mask: EVENT_NEW_SOLICITATIONS,
            enabled: false,
        },
    )
    .await
    .unwrap();

    let for_failed = webhook_config_repo::list_subscribed(&pool, EVENT_PULL_FAILED)
        .await
        .unwrap();
    assert_eq!(for_failed.len(), 1);
    assert_eq!(for_failed[0].name, "ops");
    assert!(for_failed[0].subscribes_to(EVENT_PULL_FAILED));

    // The solicitations subscriber is disabled — no fan-out.
    let for_solicit = webhook_config_repo::list_subscribed(&pool, EVENT_NEW_SOLICITATIONS)
        .await
        .unwrap();
    assert!(for_solicit.is_empty());
}

#[tokio::test]
async fn webhook_update_and_delete() {
    let pool = fresh_pool().await;
    let row = webhook_config_repo::insert(
        &pool,
        NewWebhookConfig {
            name: "w".into(),
            url: "https://x".into(),
            event_mask: EVENT_PULL_SUCCEEDED,
            enabled: true,
        },
    )
    .await
    .unwrap();
    webhook_config_repo::update(
        &pool,
        row.id,
        webhook_config_repo::WebhookConfigUpdate {
            name: "w".into(),
            url: "https://x".into(),
            event_mask: EVENT_PULL_SUCCEEDED | EVENT_PULL_FAILED,
            enabled: true,
        },
    )
    .await
    .unwrap();
    let got = webhook_config_repo::get(&pool, row.id)
        .await
        .unwrap()
        .unwrap();
    assert!(got.subscribes_to(EVENT_PULL_FAILED));

    webhook_config_repo::delete(&pool, row.id).await.unwrap();
    assert!(matches!(
        webhook_config_repo::delete(&pool, row.id).await,
        Err(DbError::NotFound)
    ));
}

// -------- pull_list --------

#[tokio::test]
async fn pull_list_add_get_and_duplicate_rejected() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool, "Saga").await;
    pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id,
            start_issue: Some("5".into()),
        },
    )
    .await
    .unwrap();
    let row = pull_list_repo::get(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.start_issue.as_deref(), Some("5"));
    assert!(!row.paused);
    assert_eq!(row.failure_count, 0);

    // UNIQUE(series_id) — a second add for the same series errors.
    assert!(pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id,
            start_issue: None
        }
    )
    .await
    .is_err());
}

#[tokio::test]
async fn pull_list_active_excludes_paused() {
    let pool = fresh_pool().await;
    let a = seed_series(&pool, "A").await;
    let b = seed_series(&pool, "B").await;
    pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id: a,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id: b,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    pull_list_repo::set_paused(&pool, b, true).await.unwrap();

    let active = pull_list_repo::list_active(&pool).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].series_id, a);
    assert_eq!(pull_list_repo::list_all(&pool).await.unwrap().len(), 2);
}

#[tokio::test]
async fn pull_list_failure_count_increments_then_resets_on_success() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool, "Saga").await;
    pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id,
            start_issue: None,
        },
    )
    .await
    .unwrap();

    pull_list_repo::mark_attempt_failed(&pool, series_id)
        .await
        .unwrap();
    pull_list_repo::mark_attempt_failed(&pool, series_id)
        .await
        .unwrap();
    let row = pull_list_repo::get(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.failure_count, 2);
    assert!(row.last_pull_attempt_at.is_some());
    assert!(row.last_successful_pull_at.is_none());

    // A successful pull zeroes failure_count and stamps the success ts.
    pull_list_repo::mark_attempt_succeeded(&pool, series_id)
        .await
        .unwrap();
    let row = pull_list_repo::get(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.failure_count, 0);
    assert!(row.last_successful_pull_at.is_some());
}

#[tokio::test]
async fn pull_list_remove_and_cascade_on_series_delete() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool, "Saga").await;
    pull_list_repo::add(
        &pool,
        NewPullEntry {
            series_id,
            start_issue: None,
        },
    )
    .await
    .unwrap();
    pull_list_repo::remove(&pool, series_id).await.unwrap();
    assert!(pull_list_repo::get(&pool, series_id)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        pull_list_repo::remove(&pool, series_id).await,
        Err(DbError::NotFound)
    ));
}

// -------- pull_attempts --------

fn new_attempt(series_id: i64, issue_id: i64, status: &str) -> NewPullAttempt {
    NewPullAttempt {
        series_id,
        issue_id,
        indexer_id: None,
        release_id: Some("guid-abc".into()),
        status: status.into(),
        error_message: None,
        retry_count: 0,
    }
}

#[tokio::test]
async fn pull_attempt_insert_and_list_for_issue() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool, "Saga").await;
    let issue_id = seed_issue(&pool, series_id, "1").await;
    pull_attempt_repo::insert(&pool, new_attempt(series_id, issue_id, "submitted"))
        .await
        .unwrap();
    pull_attempt_repo::insert(&pool, new_attempt(series_id, issue_id, "failed"))
        .await
        .unwrap();
    let attempts = pull_attempt_repo::list_for_issue(&pool, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
}

#[tokio::test]
async fn pull_attempt_in_flight_detection() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool, "Saga").await;
    let issue_id = seed_issue(&pool, series_id, "1").await;
    assert!(
        !pull_attempt_repo::has_in_flight_attempt(&pool, series_id, issue_id)
            .await
            .unwrap()
    );

    pull_attempt_repo::insert(&pool, new_attempt(series_id, issue_id, "submitted"))
        .await
        .unwrap();
    assert!(
        pull_attempt_repo::has_in_flight_attempt(&pool, series_id, issue_id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn pull_attempt_mark_grabbed_transitions_all_in_flight() {
    // The race case from the Step 6 kickoff resolution: 2+ in-flight
    // attempts for the same issue all settle to 'grabbed'.
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool, "Saga").await;
    let issue_id = seed_issue(&pool, series_id, "1").await;
    pull_attempt_repo::insert(&pool, new_attempt(series_id, issue_id, "pending"))
        .await
        .unwrap();
    pull_attempt_repo::insert(&pool, new_attempt(series_id, issue_id, "submitted"))
        .await
        .unwrap();
    // A prior failed attempt must NOT be touched.
    pull_attempt_repo::insert(&pool, new_attempt(series_id, issue_id, "failed"))
        .await
        .unwrap();

    let transitioned = pull_attempt_repo::mark_grabbed_for_issue(&pool, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(transitioned, 2, "both in-flight rows, not the failed one");

    let attempts = pull_attempt_repo::list_for_issue(&pool, series_id, issue_id)
        .await
        .unwrap();
    let grabbed = attempts.iter().filter(|a| a.status == "grabbed").count();
    let failed = attempts.iter().filter(|a| a.status == "failed").count();
    assert_eq!(grabbed, 2);
    assert_eq!(failed, 1);
    assert!(
        !pull_attempt_repo::has_in_flight_attempt(&pool, series_id, issue_id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn pull_attempt_update_status() {
    let pool = fresh_pool().await;
    let series_id = seed_series(&pool, "Saga").await;
    let issue_id = seed_issue(&pool, series_id, "1").await;
    let row = pull_attempt_repo::insert(&pool, new_attempt(series_id, issue_id, "pending"))
        .await
        .unwrap();
    pull_attempt_repo::update_status(&pool, row.id, "failed", Some("indexer timeout"))
        .await
        .unwrap();
    let attempts = pull_attempt_repo::list_for_issue(&pool, series_id, issue_id)
        .await
        .unwrap();
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(
        attempts[0].error_message.as_deref(),
        Some("indexer timeout")
    );
}

// -------- cv_release_cache --------

#[tokio::test]
async fn release_cache_upsert_get_and_replace() {
    let pool = fresh_pool().await;
    release_cache_repo::upsert(
        &pool,
        NewReleaseCacheEntry {
            date_from: "2026-05-20".into(),
            date_to: "2026-05-26".into(),
            publisher: String::new(),
            payload_json: r#"{"v":1}"#.into(),
        },
    )
    .await
    .unwrap();
    let got = release_cache_repo::get(&pool, "2026-05-20", "2026-05-26", "")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.payload_json, r#"{"v":1}"#);

    // Re-cache the same key — payload replaced, still one row.
    release_cache_repo::upsert(
        &pool,
        NewReleaseCacheEntry {
            date_from: "2026-05-20".into(),
            date_to: "2026-05-26".into(),
            publisher: String::new(),
            payload_json: r#"{"v":2}"#.into(),
        },
    )
    .await
    .unwrap();
    let got = release_cache_repo::get(&pool, "2026-05-20", "2026-05-26", "")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.payload_json, r#"{"v":2}"#);
}

#[tokio::test]
async fn release_cache_get_miss_is_none() {
    let pool = fresh_pool().await;
    assert!(release_cache_repo::get(&pool, "x", "y", "")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn release_cache_prune_stale() {
    let pool = fresh_pool().await;
    release_cache_repo::upsert(
        &pool,
        NewReleaseCacheEntry {
            date_from: "a".into(),
            date_to: "b".into(),
            publisher: String::new(),
            payload_json: "{}".into(),
        },
    )
    .await
    .unwrap();
    // Cutoff far in the future — the just-written row is "stale".
    let pruned = release_cache_repo::prune_stale(&pool, datetime!(2099-01-01 0:00))
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    assert!(release_cache_repo::get(&pool, "a", "b", "")
        .await
        .unwrap()
        .is_none());
}
