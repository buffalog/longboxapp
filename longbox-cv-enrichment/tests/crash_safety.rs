//! Crash-safety + four-confirmation integration tests.
//!
//! Tests use `:memory:` DB pools and exercise the worker's per-
//! attempt transaction shape directly — `attempt_one` is not
//! exposed, so we call `commit_merge` equivalents by reusing the
//! repo primitives the way the worker does. The transaction-
//! boundary property is the load-bearing assertion: a torn middle
//! state can't exist because the transaction wraps every write.

mod common;

use std::collections::HashSet;

use common::{ensure_library_root, fresh_pool, seed_shallow_series_with_issues};
use longbox_db::{cv_volume_cache_repo, issue_repo, series_repo, NewIssue};

// ===== Q4 confirmation: set_cv_id rows-affected is a real outcome =====

/// The race-guard predicate (`WHERE id = ? AND cv_id IS NULL`)
/// MUST refuse the second writer, returning rows_affected=0.
/// Caller must treat that as a real outcome — not silently
/// succeed.
#[tokio::test]
async fn set_cv_id_race_returns_zero_rows_when_already_linked() {
    let pool = fresh_pool().await;
    let series_id = seed_shallow_series_with_issues(&pool, "Saga", Some(2012), &["1"]).await;

    // First writer wins.
    let first = series_repo::set_cv_id(&pool, series_id, 1234)
        .await
        .unwrap();
    assert_eq!(first, 1);

    // Second writer (different cv_id) — predicate refuses.
    let second = series_repo::set_cv_id(&pool, series_id, 5678)
        .await
        .unwrap();
    assert_eq!(
        second, 0,
        "race-guard must refuse the second writer (cv_id is already set)"
    );

    // First writer's cv_id stands.
    let s = series_repo::find_by_id(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.cv_id, Some(1234));
}

// ===== Q5 confirmation: orphan SELECT is post-upsert-within-tx =====

/// The orphan-numbers query is correct ONLY when run after the
/// upserts, inside the same transaction. The synthesized rows
/// that get matched-and-updated by number have cv_issue_id set
/// by the upsert, so they correctly fall out of the orphan
/// filter. Pre-upsert ordering would count every synthesized
/// row as orphan.
#[tokio::test]
async fn orphan_numbers_post_upsert_excludes_matched_rows() {
    let pool = fresh_pool().await;
    // Synthesized issues 1, 2, 3, 99 (all cv_issue_id NULL).
    let series_id =
        seed_shallow_series_with_issues(&pool, "Saga", Some(2012), &["1", "2", "3", "99"]).await;

    // Open transaction, run the upserts CV would for #1, #2, #3
    // (NOT #99 — that's the orphan-stranded number), then query
    // orphans inside the same transaction.
    let mut tx = pool.begin().await.unwrap();
    for n in ["1", "2", "3"] {
        issue_repo::upsert_by_series_id_and_number_with_cv_fields(
            &mut *tx,
            NewIssue {
                series_id,
                cv_issue_id: Some(n.parse::<i64>().unwrap() + 1000),
                metron_issue_id: None,
                number: n.into(),
                title: Some(format!("CV #{n}")),
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }
    let orphans = series_repo::list_orphan_synthesized_numbers(&mut *tx, series_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(orphans, vec!["99"], "only #99 should remain synthesized");
}

// ===== Q11 confirmation: collision_disabled records explicitly =====

/// `record_enrichment_outcome` writes the outcome string + the
/// timestamp regardless of which AttemptOutcome variant; the
/// `collision_disabled` outcome is recorded as a positive write
/// (with an attempt timestamp) so 6c.3's bucketed report sees the
/// series as "attempted-and-routed-to-manual" rather than as
/// silently filtered out.
#[tokio::test]
async fn collision_disabled_records_explicit_outcome_with_timestamp() {
    let pool = fresh_pool().await;
    let series_id = seed_shallow_series_with_issues(&pool, "Sex", None, &["1"]).await;

    series_repo::record_enrichment_outcome(&pool, series_id, "collision_disabled", None)
        .await
        .unwrap();

    let s = series_repo::find_by_id(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert!(s.cv_id.is_none(), "still shallow");
    // Re-fetch the row with the enrichment columns visible. They're
    // not in SeriesRow today, so query directly.
    let row = sqlx::query!(
        r#"SELECT last_enrichment_attempt_at AS "ts: time::PrimitiveDateTime",
                  last_enrichment_outcome AS "outcome: String"
           FROM series WHERE id = ?"#,
        series_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        row.ts.is_some(),
        "attempt timestamp must be set — proves the pre-filter recorded explicitly"
    );
    assert_eq!(
        row.outcome.as_deref(),
        Some("collision_disabled"),
        "outcome string visible to the bucketed report"
    );
}

// ===== Q12 — three crash-safety timing classes =====

/// **Pre-fetch cancellation.** No DB writes have happened. Series
/// stays shallow, attempt timestamp NOT set (the outcome write
/// only happens after fetch in the production code path, so this
/// test simulates "we never even ran outcome-recording"). Next
/// cycle treats it like an unattempted candidate.
#[tokio::test]
async fn cancelled_pre_fetch_leaves_series_shallow_no_outcome_recorded() {
    let pool = fresh_pool().await;
    let series_id = seed_shallow_series_with_issues(&pool, "Saga", Some(2012), &["1", "2"]).await;

    // Simulate worker exiting before phase 1 runs to completion.
    // No outcome write, no transaction.

    let s = series_repo::find_by_id(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert!(s.cv_id.is_none());
    let row = sqlx::query!(
        r#"SELECT last_enrichment_attempt_at AS "ts: time::PrimitiveDateTime",
                  last_enrichment_outcome AS "outcome: String"
           FROM series WHERE id = ?"#,
        series_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.ts.is_none());
    assert!(row.outcome.is_none());
}

/// **Mid-transaction cancellation.** The load-bearing crash-safety
/// test, per Jeremy's note. We start a transaction, perform the
/// writes the worker would do (set_cv_id, one upsert, the outcome
/// record), then EXPLICITLY ROLL BACK to simulate cancellation
/// before commit. Assert: cv_id IS NULL, no issues have
/// cv_issue_id set, AND the attempt timestamp is unset. The third
/// assertion is the subtle one — it proves the outcome-recording
/// is genuinely inside the transaction boundary, not adjacent to
/// it. If the timestamp survived the rollback, you'd have a series
/// that looks attempted-and-cooling-down when it was actually
/// never completed.
#[tokio::test]
async fn cancelled_mid_transaction_rolls_back_outcome_with_data() {
    let pool = fresh_pool().await;
    let series_id = seed_shallow_series_with_issues(&pool, "Saga", Some(2012), &["1", "2"]).await;

    // Pre-state snapshot.
    let pre_issues = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    assert_eq!(pre_issues.len(), 2);
    assert!(pre_issues.iter().all(|i| i.cv_issue_id.is_none()));

    // Open transaction, do the worker's phase-2 writes.
    let mut tx = pool.begin().await.unwrap();

    let promoted = series_repo::set_cv_id(&mut *tx, series_id, 9999)
        .await
        .unwrap();
    assert_eq!(promoted, 1, "set_cv_id succeeded inside tx");

    // 6c.5: the merge also persists publisher / description /
    // cover_url from the fetched CvVolumeDetail. These must roll
    // back atomically with the rest of the transaction on
    // cancellation — proving the descriptive payload is inside the
    // transaction boundary, not adjacent to it.
    series_repo::update_series_volume_detail(
        &mut *tx,
        series_id,
        Some("Image Comics"),
        Some("A sweeping space-fantasy saga."),
        Some("https://cv/cover.jpg"),
        None,
    )
    .await
    .unwrap();

    issue_repo::upsert_by_series_id_and_number_with_cv_fields(
        &mut *tx,
        NewIssue {
            series_id,
            cv_issue_id: Some(7001),
            metron_issue_id: None,
            number: "1".into(),
            title: Some("CV title".into()),
            cover_date: None,
            summary: None,
            cover_url: None,
        },
    )
    .await
    .unwrap();

    series_repo::record_enrichment_outcome(&mut *tx, series_id, "matched", Some("score=1.000"))
        .await
        .unwrap();

    // SIMULATE CANCELLATION: roll back instead of commit.
    tx.rollback().await.unwrap();

    // (1) series.cv_id rolled back.
    let s = series_repo::find_by_id(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        s.cv_id.is_none(),
        "set_cv_id rolled back — series stays shallow"
    );

    // (2) issues' cv_issue_id rolled back.
    let post_issues = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    assert!(
        post_issues.iter().all(|i| i.cv_issue_id.is_none()),
        "all issue rows rolled back — no CV fields survive"
    );

    // (3) THE LOAD-BEARING ASSERTION: attempt timestamp rolled back.
    // If this fails, outcome-recording is happening outside the
    // transaction boundary and the "torn state can't exist" claim
    // is wrong.
    let row = sqlx::query!(
        r#"SELECT last_enrichment_attempt_at AS "ts: time::PrimitiveDateTime",
                  last_enrichment_outcome AS "outcome: String"
           FROM series WHERE id = ?"#,
        series_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        row.ts.is_none(),
        "outcome timestamp must roll back with the rest of the transaction"
    );
    assert!(row.outcome.is_none(), "outcome string must roll back too");

    // (4) 6c.5: publisher / description / cover_url rolled back too.
    // The three descriptive fields must be NULL — if any survives
    // the rollback, update_series_volume_detail is escaping the
    // transaction boundary.
    assert!(
        s.publisher.is_none(),
        "publisher must roll back with the transaction"
    );
    assert!(
        s.description.is_none(),
        "description must roll back with the transaction"
    );
    assert!(
        s.cover_url.is_none(),
        "cover_url must roll back with the transaction"
    );
}

/// **Post-commit durability.** A transaction that successfully
/// commits IS durable, including across simulated process restart
/// (we just open a fresh connection to the same DB).
#[tokio::test]
async fn committed_enrichment_is_durable() {
    let pool = fresh_pool().await;
    let series_id = seed_shallow_series_with_issues(&pool, "Saga", Some(2012), &["1", "2"]).await;

    let mut tx = pool.begin().await.unwrap();
    series_repo::set_cv_id(&mut *tx, series_id, 12345)
        .await
        .unwrap();
    // 6c.5: persist the descriptive volume detail inside the same
    // transaction. Asserted below to verify the three fields land
    // durably alongside cv_id and the per-issue upserts.
    series_repo::update_series_volume_detail(
        &mut *tx,
        series_id,
        Some("Image Comics"),
        Some("A sweeping space-fantasy saga."),
        Some("https://cv/cover.jpg"),
        None,
    )
    .await
    .unwrap();
    for n in ["1", "2"] {
        issue_repo::upsert_by_series_id_and_number_with_cv_fields(
            &mut *tx,
            NewIssue {
                series_id,
                cv_issue_id: Some(n.parse::<i64>().unwrap() + 5000),
                metron_issue_id: None,
                number: n.into(),
                title: Some(format!("CV #{n}")),
                cover_date: None,
                summary: None,
                cover_url: None,
            },
        )
        .await
        .unwrap();
    }
    series_repo::record_enrichment_outcome(&mut *tx, series_id, "matched", Some("score=1.000"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Re-acquire (simulates fresh connection / process restart).
    let s = series_repo::find_by_id(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.cv_id, Some(12345));
    // 6c.5: all three descriptive fields land durably.
    assert_eq!(s.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(
        s.description.as_deref(),
        Some("A sweeping space-fantasy saga.")
    );
    assert_eq!(s.cover_url.as_deref(), Some("https://cv/cover.jpg"));
    let issues = issue_repo::list_by_series(&pool, series_id).await.unwrap();
    let cv_ids: HashSet<Option<i64>> = issues.iter().map(|i| i.cv_issue_id).collect();
    assert!(cv_ids.contains(&Some(5001)));
    assert!(cv_ids.contains(&Some(5002)));
}

// ===== assert_schema — defense around migration gremlin =====

#[tokio::test]
async fn assert_schema_passes_on_fresh_migrated_db() {
    let pool = fresh_pool().await;
    let result = longbox_cv_enrichment::worker::assert_schema(&pool).await;
    assert!(result.is_ok(), "fresh-migrated db must pass schema check");
}

// ===== list_shallow_for_enrichment + collision flag =====

#[tokio::test]
async fn shallow_query_marks_collision_flag_correctly() {
    let pool = fresh_pool().await;
    let _ = ensure_library_root(&pool).await;
    // Two same-titled shallow series (collision).
    let a = seed_shallow_series_with_issues(&pool, "Sex", None, &["1"]).await;
    let b = seed_shallow_series_with_issues(&pool, "Sex", None, &["2"]).await;
    // One title-unique year-unknown.
    let c = seed_shallow_series_with_issues(&pool, "Invincible", None, &["1"]).await;

    let candidates = series_repo::list_shallow_for_enrichment(
        &pool,
        7,
        longbox_db::CandidateSelectionMode::PriorityOrder,
    )
    .await
    .unwrap();

    let by_id: std::collections::HashMap<i64, bool> = candidates
        .iter()
        .map(|c| (c.series_id, c.catalog_title_collision))
        .collect();
    assert_eq!(by_id.get(&a), Some(&true), "Sex (a) flagged collision");
    assert_eq!(by_id.get(&b), Some(&true), "Sex (b) flagged collision");
    assert_eq!(by_id.get(&c), Some(&false), "Invincible NOT flagged");
}

// ===== 6c.5: update_series_volume_detail + list_volume_refresh_candidates =====

/// Standalone write path the refresh pass uses (not the merge tx).
/// Verifies all three descriptive fields are persisted in a single
/// UPDATE and rows-affected is 1 for an existing series.
#[tokio::test]
async fn update_series_volume_detail_writes_all_three_fields() {
    let pool = fresh_pool().await;
    let series_id = seed_shallow_series_with_issues(&pool, "Saga", Some(2012), &["1"]).await;
    series_repo::set_cv_id(&pool, series_id, 46568)
        .await
        .unwrap();

    let rows = series_repo::update_series_volume_detail(
        &pool,
        series_id,
        Some("Image Comics"),
        Some("Space fantasy."),
        Some("https://cv/saga-cover.jpg"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(rows, 1, "exactly one series row updated");

    let s = series_repo::find_by_id(&pool, series_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(s.description.as_deref(), Some("Space fantasy."));
    assert_eq!(s.cover_url.as_deref(), Some("https://cv/saga-cover.jpg"));
}

/// The refresh-pass candidate query returns CV-linked series with
/// `publisher IS NULL` only — shallow series and CV-linked series
/// whose publisher is already populated are excluded.
#[tokio::test]
async fn volume_refresh_candidates_excludes_shallow_and_publisher_populated() {
    let pool = fresh_pool().await;

    // (a) Shallow series — must NOT appear (cv_id IS NULL).
    let _shallow = seed_shallow_series_with_issues(&pool, "Shallow", None, &["1"]).await;

    // (b) CV-linked, publisher NULL — IS a refresh candidate.
    let needs_refresh =
        seed_shallow_series_with_issues(&pool, "Needs Refresh", Some(2020), &["1"]).await;
    series_repo::set_cv_id(&pool, needs_refresh, 1111)
        .await
        .unwrap();

    // (c) CV-linked, publisher populated — must NOT appear.
    let already_done =
        seed_shallow_series_with_issues(&pool, "Already Done", Some(2021), &["1"]).await;
    series_repo::set_cv_id(&pool, already_done, 2222)
        .await
        .unwrap();
    series_repo::update_series_volume_detail(
        &pool,
        already_done,
        Some("Some Publisher"),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let candidates = series_repo::list_volume_refresh_candidates(&pool)
        .await
        .unwrap();
    let ids: HashSet<i64> = candidates.iter().map(|c| c.series_id).collect();
    assert!(
        ids.contains(&needs_refresh),
        "CV-linked + publisher NULL must be a candidate"
    );
    assert!(
        !ids.contains(&already_done),
        "CV-linked + publisher set must NOT be a candidate"
    );
    assert_eq!(
        candidates.len(),
        1,
        "exactly the one CV-linked-publisher-NULL series, nothing else"
    );
}

// ===== Item E v2: cv_volume_cache (queue, fill, list_pending) =====

/// bulk_queue_pending is the calendar's queue producer. INSERT OR
/// IGNORE means a duplicate cv_volume_id is a clean no-op rather
/// than a failure — the calendar can call this every request without
/// caring whether the ids are already queued (or already filled).
#[tokio::test]
async fn cv_volume_cache_queue_is_idempotent_on_duplicate_ids() {
    let pool = fresh_pool().await;

    // First insert — all four ids are new.
    let inserted1 = cv_volume_cache_repo::bulk_queue_pending(&pool, &[100, 200, 300, 400])
        .await
        .unwrap();
    assert_eq!(inserted1, 4);

    // Second call with overlap — only the truly-new ids are inserted.
    let inserted2 = cv_volume_cache_repo::bulk_queue_pending(&pool, &[300, 400, 500, 600])
        .await
        .unwrap();
    assert_eq!(inserted2, 2, "300 + 400 are no-ops; only 500 + 600 insert");

    // All six end up in the table.
    let row = cv_volume_cache_repo::find_by_id(&pool, 600).await.unwrap();
    assert!(row.is_some());
    let row = row.unwrap();
    assert!(row.fetched_at.is_none(), "freshly queued — fetched_at NULL");
    assert!(row.publisher.is_none());
}

/// list_pending only returns rows with fetched_at IS NULL. A
/// successfully filled row drops out of the pending set even if its
/// publisher came back NULL from CV — fetched_at, not publisher
/// non-null, is the "I've done the work" signal.
#[tokio::test]
async fn cv_volume_cache_list_pending_excludes_fetched_rows() {
    let pool = fresh_pool().await;

    // Queue three.
    cv_volume_cache_repo::bulk_queue_pending(&pool, &[111, 222, 333])
        .await
        .unwrap();

    // Fill 222 with a real publisher.
    cv_volume_cache_repo::mark_fetched(
        &pool,
        222,
        Some("Image Comics"),
        Some("desc"),
        Some(2012),
        Some("https://cv/cover-222.jpg"),
    )
        .await
        .unwrap();

    // Fill 333 with a NULL publisher (CV returned no publisher data).
    // fetched_at MUST still be set — otherwise the worker re-attempts
    // forever on volumes that legitimately have no publisher field.
    cv_volume_cache_repo::mark_fetched(&pool, 333, None, None, None, None)
        .await
        .unwrap();

    let pending = cv_volume_cache_repo::list_pending(&pool).await.unwrap();
    let pending_ids: HashSet<i64> = pending.iter().map(|p| p.cv_volume_id).collect();
    assert!(
        pending_ids.contains(&111),
        "111 still pending — fetched_at is NULL"
    );
    assert!(
        !pending_ids.contains(&222),
        "222 fetched with publisher — drops out of pending"
    );
    assert!(
        !pending_ids.contains(&333),
        "333 fetched with NULL publisher — also drops out (fetched_at is set)"
    );
    assert_eq!(pending.len(), 1);
}

/// mark_fetched persists publisher + description + start_year together
/// in a single UPDATE and sets fetched_at. The repo behavior is what
/// the worker's cache_fill_one relies on.
#[tokio::test]
async fn cv_volume_cache_mark_fetched_writes_all_three_fields() {
    let pool = fresh_pool().await;

    cv_volume_cache_repo::bulk_queue_pending(&pool, &[42])
        .await
        .unwrap();
    let rows = cv_volume_cache_repo::mark_fetched(
        &pool,
        42,
        Some("Dark Horse"),
        Some("Long description."),
        Some(2024),
        Some("https://cv/cover-42.jpg"),
    )
    .await
    .unwrap();
    assert_eq!(rows, 1);

    let row = cv_volume_cache_repo::find_by_id(&pool, 42)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.publisher.as_deref(), Some("Dark Horse"));
    assert_eq!(row.description.as_deref(), Some("Long description."));
    assert_eq!(row.start_year, Some(2024));
    assert_eq!(row.cover_url.as_deref(), Some("https://cv/cover-42.jpg"));
    assert!(row.fetched_at.is_some(), "fetched_at set on success");
}

/// list_pending order: FIFO by first_seen_at. Older queue entries
/// drain first — more likely to be hit by repeat calendar loads, so
/// the user-visible "Unknown Publisher" stays in front for the
/// shortest time.
#[tokio::test]
async fn cv_volume_cache_pending_orders_oldest_first() {
    let pool = fresh_pool().await;

    // Queue in two batches with a delay to force distinct first_seen_at.
    cv_volume_cache_repo::bulk_queue_pending(&pool, &[1000, 1001])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    cv_volume_cache_repo::bulk_queue_pending(&pool, &[1002, 1003])
        .await
        .unwrap();

    let pending = cv_volume_cache_repo::list_pending(&pool).await.unwrap();
    let ids: Vec<i64> = pending.iter().map(|p| p.cv_volume_id).collect();
    // The first batch (1000, 1001) must come before the second
    // batch (1002, 1003). Within a batch, order is by cv_volume_id ASC.
    assert_eq!(ids, vec![1000, 1001, 1002, 1003]);
}
