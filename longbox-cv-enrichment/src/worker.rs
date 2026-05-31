//! The enrichment worker — continuous loop + per-series attempt.
//!
//! Lifecycle mirrors `pull::schedule::PullHandle`: `spawn` returns
//! an [`EnrichmentHandle`] holding an `mpsc::Sender<()>` for manual
//! trigger + an `Arc<AtomicBool>` for "currently running" reporting.
//! The web layer parks the handle in `AppState`; process exit
//! drops it, the worker's `mpsc::Receiver::recv()` returns `None`,
//! and the loop exits cleanly.
//!
//! Crash-safety relies on the per-attempt transaction shape — see
//! [`crate`] module-level docs and the structured tests in
//! `longbox-cv-enrichment/tests/`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use longbox_comicvine::{
    enrichment::pick_volume, ComicVineClient, CvIssueDetail, CvVolumeDetail,
    EnrichmentCandidateInput, PickOutcome, SeriesSearchResult,
};
use longbox_db::{
    cv_volume_cache_repo, issue_repo, series_repo, CandidateSelectionMode, CvVolumePending,
    NewIssue, Pool, ShallowEnrichmentCandidate, VolumeRefreshCandidate,
};
use tokio::sync::mpsc;

use crate::background::BackgroundCvClient;
use crate::config::EnrichmentConfig;
use crate::outcome::AttemptOutcome;

/// Handle to a running enrichment worker. Cheap to clone — the web
/// layer parks one in `AppState` for the manual "Run now" route.
#[derive(Clone)]
pub struct EnrichmentHandle {
    trigger: mpsc::Sender<()>,
    running: Arc<AtomicBool>,
}

impl EnrichmentHandle {
    /// Request an immediate cycle. Returns `false` when a cycle is
    /// already in flight (the route surfaces that as 409) or the
    /// worker has shut down; `true` when the request was accepted
    /// or coalesced into a queued one.
    pub fn request_run(&self) -> bool {
        if self.running.load(Ordering::SeqCst) {
            return false;
        }
        !matches!(
            self.trigger.try_send(()),
            Err(mpsc::error::TrySendError::Closed(())),
        )
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Spawn the worker. The worker performs the startup migration
/// assertion (see [`assert_schema`]) before doing any other work;
/// if assertion fails, the worker refuses to start, logs at ERROR,
/// and the returned handle's `request_run` will be a no-op.
pub fn spawn(db: Pool, inner_cv: Arc<ComicVineClient>) -> EnrichmentHandle {
    let running = Arc::new(AtomicBool::new(false));
    let (trigger, rx) = mpsc::channel::<()>(1);
    let handle = EnrichmentHandle {
        trigger,
        running: Arc::clone(&running),
    };
    tokio::spawn(worker_loop(db, inner_cv, running, rx));
    handle
}

async fn worker_loop(
    db: Pool,
    inner_cv: Arc<ComicVineClient>,
    running: Arc<AtomicBool>,
    mut trigger: mpsc::Receiver<()>,
) {
    // Startup migration assertion. If columns are missing, log ERROR
    // and drain the trigger channel forever — the handle's
    // request_run still returns true (channel open), but the worker
    // does no work. Defense around the deferred
    // migration-not-applied-on-startup item.
    match assert_schema(&db).await {
        Ok(()) => {
            tracing::info!(
                target: "longbox_cv_enrichment",
                "cv_enrichment.started"
            );
        }
        Err(missing) => {
            tracing::error!(
                target: "longbox_cv_enrichment",
                missing = ?missing,
                "cv_enrichment.start_refused (migration_missing)"
            );
            // Park forever, draining triggers harmlessly.
            while trigger.recv().await.is_some() {
                tracing::warn!(
                    target: "longbox_cv_enrichment",
                    "cv_enrichment.trigger_ignored (worker refused to start)"
                );
            }
            return;
        }
    }

    loop {
        // Wait for a manual trigger OR run continuously if there is
        // backlog. Heuristic: if max_run > 0 and a candidate cycle
        // produces 0 attempts, sleep 5 minutes before re-checking;
        // otherwise loop immediately and let the per-attempt
        // background gate enforce 30s spacing.
        let config = match EnrichmentConfig::load(&db).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "longbox_cv_enrichment",
                    error = %e,
                    "cv_enrichment.config_load_failed"
                );
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
        };

        // Bug 4a kill switch. Disabled → idle 60s before re-reading
        // (so flipping the setting via SQL takes effect within ~1
        // minute, no restart needed).
        if !config.enabled {
            tracing::debug!(
                target: "longbox_cv_enrichment",
                "cv_enrichment.disabled (cv_enrichment_enabled=false)"
            );
            tokio::select! {
                msg = trigger.recv() => {
                    if msg.is_none() {
                        tracing::info!(target: "longbox_cv_enrichment", "cv_enrichment.stopped");
                        return;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
            continue;
        }

        let bg = BackgroundCvClient::new(
            Arc::clone(&inner_cv),
            Duration::from_secs(config.request_interval_seconds),
        );

        let candidates = match list_candidates(&db, &config).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "longbox_cv_enrichment",
                    error = %e,
                    "cv_enrichment.candidate_query_failed"
                );
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
        };

        if candidates.is_empty() {
            // Primary (shallow) set exhausted. Two secondary passes
            // remain, in this priority order:
            //
            //   1. cache-fill  — Item E v2 v2.1: cv_volume_cache pending
            //      rows queued by recent calendar responses. Backs the
            //      publisher grouping in the *calendar view the user is
            //      currently looking at*. If this trails by hours, the
            //      calendar shows "Unknown Publisher" for high-profile
            //      non-catalog vols (Batman, Superman, etc.) — visibly
            //      broken UX.
            //
            //   2. refresh     — 6c.5: CV-linked series with NULL
            //      publisher. Populates publisher on already-owned
            //      series (library detail view, lower urgency). Drains
            //      after cache-fill so a fresh calendar load doesn't
            //      sit behind hours of library backlog.
            //
            // Priority flipped on 2026-05-31 after the initial Item E
            // v2 deploy showed cache-fill queued for ~2.5 hours behind
            // a 187-series refresh backlog. Cache-fill first means
            // first-load -> publishers visible in ~50 min instead of
            // ~150.

            let cache_fill = match cv_volume_cache_repo::list_pending(&db).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        target: "longbox_cv_enrichment",
                        error = %e,
                        "cv_enrichment.cache_fill_candidate_query_failed"
                    );
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
            };

            if !cache_fill.is_empty() {
                running.store(true, Ordering::SeqCst);
                let attempts_this_cycle = run_cache_fill_cycle(&db, &bg, &config, cache_fill).await;
                running.store(false, Ordering::SeqCst);

                tracing::info!(
                    target: "longbox_cv_enrichment",
                    attempts = attempts_this_cycle,
                    max_run = config.max_run,
                    pass = "cache_fill",
                    "cv_enrichment.cycle_complete"
                );

                if trigger.is_closed() {
                    tracing::info!(target: "longbox_cv_enrichment", "cv_enrichment.stopped");
                    return;
                }
                continue;
            }

            // Cache-fill empty — drain refresh next.
            let refresh = match series_repo::list_volume_refresh_candidates(&db).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        target: "longbox_cv_enrichment",
                        error = %e,
                        "cv_enrichment.refresh_candidate_query_failed"
                    );
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
            };

            if refresh.is_empty() {
                tracing::debug!(
                    target: "longbox_cv_enrichment",
                    "cv_enrichment.idle (no shallow, cache-fill, or refresh backlog)"
                );
                // Idle: wait for trigger OR 5-minute timeout.
                tokio::select! {
                    msg = trigger.recv() => {
                        if msg.is_none() {
                            tracing::info!(target: "longbox_cv_enrichment", "cv_enrichment.stopped");
                            return;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(300)) => {}
                }
                continue;
            }

            running.store(true, Ordering::SeqCst);
            let attempts_this_cycle = run_refresh_cycle(&db, &bg, &config, refresh).await;
            running.store(false, Ordering::SeqCst);

            tracing::info!(
                target: "longbox_cv_enrichment",
                attempts = attempts_this_cycle,
                max_run = config.max_run,
                pass = "refresh",
                "cv_enrichment.cycle_complete"
            );

            if trigger.is_closed() {
                tracing::info!(target: "longbox_cv_enrichment", "cv_enrichment.stopped");
                return;
            }
            continue;
        }

        running.store(true, Ordering::SeqCst);
        let attempts_this_cycle = run_cycle(&db, &bg, &config, candidates).await;
        running.store(false, Ordering::SeqCst);

        tracing::info!(
            target: "longbox_cv_enrichment",
            attempts = attempts_this_cycle,
            max_run = config.max_run,
            pass = "shallow",
            "cv_enrichment.cycle_complete"
        );

        // Did the channel close while we were working? Exit.
        if trigger.is_closed() {
            tracing::info!(target: "longbox_cv_enrichment", "cv_enrichment.stopped");
            return;
        }
    }
}

/// Per-cycle candidate selection. Honors the first-run sample-mode
/// gate by switching the candidate query's mode.
async fn list_candidates(
    db: &Pool,
    config: &EnrichmentConfig,
) -> Result<Vec<ShallowEnrichmentCandidate>, longbox_db::DbError> {
    let mode = if config.first_run_sample_mode {
        CandidateSelectionMode::FirstRunSample {
            easy_quota: config.first_run_easy_quota,
            discipline_quota: config.first_run_discipline_quota,
            collision_quota: config.first_run_collision_quota,
        }
    } else {
        CandidateSelectionMode::PriorityOrder
    };
    series_repo::list_shallow_for_enrichment(db, config.cooldown_days, mode).await
}

/// Run up to `config.max_run` per-series attempts. Returns the
/// number actually attempted.
async fn run_cycle(
    db: &Pool,
    bg: &BackgroundCvClient,
    config: &EnrichmentConfig,
    candidates: Vec<ShallowEnrichmentCandidate>,
) -> usize {
    let mut attempts = 0usize;
    for c in candidates {
        if config.max_run > 0 && attempts >= config.max_run as usize {
            break;
        }
        let outcome = attempt_one(db, bg, config, &c).await;
        log_outcome(&c, &outcome);
        attempts += 1;
    }
    attempts
}

/// Refresh pass: drain the volume-refresh candidate set (CV-linked
/// series with NULL publisher) by calling only `fetch_volume` per
/// series — one CV call vs the primary pass's three (search +
/// fetch_volume + fetch_issues). Same background-throttle, same
/// `max_run` bound; success writes publisher/description/cover_url
/// to the series row. Failures log a warn and move on (next cycle
/// re-attempts).
async fn run_refresh_cycle(
    db: &Pool,
    bg: &BackgroundCvClient,
    config: &EnrichmentConfig,
    candidates: Vec<VolumeRefreshCandidate>,
) -> usize {
    let mut attempts = 0usize;
    for c in candidates {
        if config.max_run > 0 && attempts >= config.max_run as usize {
            break;
        }
        refresh_one(db, bg, &c).await;
        attempts += 1;
    }
    attempts
}

/// Cache-fill pass: drain the cv_volume_cache pending set (rows
/// with fetched_at IS NULL, queued by calendar responses for
/// non-catalog volumes). One fetch_volume per id, writes publisher
/// / description / start_year + sets fetched_at. Same gate, same
/// max_run; no cooldown for failures (rare; next cycle re-attempts).
async fn run_cache_fill_cycle(
    db: &Pool,
    bg: &BackgroundCvClient,
    config: &EnrichmentConfig,
    candidates: Vec<CvVolumePending>,
) -> usize {
    let mut attempts = 0usize;
    for c in candidates {
        if config.max_run > 0 && attempts >= config.max_run as usize {
            break;
        }
        cache_fill_one(db, bg, c.cv_volume_id).await;
        attempts += 1;
    }
    attempts
}

/// Fetch one CV volume and write its metadata to cv_volume_cache.
/// On success, fetched_at is set so the row drops out of the
/// pending list. On failure (warn-logged), the row stays pending
/// and the next cycle re-attempts.
async fn cache_fill_one(db: &Pool, bg: &BackgroundCvClient, cv_volume_id: i64) {
    let volume = match bg.fetch_volume(cv_volume_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "longbox_cv_enrichment",
                cv_volume_id,
                error = %e,
                "cv_volume_cache.fetch_failed"
            );
            return;
        }
    };
    match cv_volume_cache_repo::mark_fetched(
        db,
        cv_volume_id,
        volume.publisher.as_deref(),
        volume.description.as_deref(),
        volume.start_year,
    )
    .await
    {
        Ok(rows) => {
            tracing::info!(
                target: "longbox_cv_enrichment",
                cv_volume_id,
                publisher = ?volume.publisher,
                start_year = ?volume.start_year,
                rows_affected = rows,
                "cv_volume_cache.filled"
            );
        }
        Err(e) => {
            tracing::warn!(
                target: "longbox_cv_enrichment",
                cv_volume_id,
                error = %e,
                "cv_volume_cache.write_failed"
            );
        }
    }
}

/// Refresh one CV-linked series: fetch the volume detail and
/// persist `publisher`, `description`, `cover_url`. No transaction
/// (single UPDATE is atomic on its own; no multi-row consistency
/// to worry about). Failures don't write any state — the candidate
/// stays in the refresh set and is re-attempted next cycle.
async fn refresh_one(
    db: &Pool,
    bg: &BackgroundCvClient,
    candidate: &VolumeRefreshCandidate,
) {
    let volume = match bg.fetch_volume(candidate.cv_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "longbox_cv_enrichment",
                series_id = candidate.series_id,
                cv_id = candidate.cv_id,
                title = %candidate.title,
                error = %e,
                "cv_enrichment.refresh_fetch_failed"
            );
            return;
        }
    };
    match series_repo::update_series_volume_detail(
        db,
        candidate.series_id,
        volume.publisher.as_deref(),
        volume.description.as_deref(),
        volume.cover_url.as_deref(),
    )
    .await
    {
        Ok(rows) => {
            tracing::info!(
                target: "longbox_cv_enrichment",
                series_id = candidate.series_id,
                cv_id = candidate.cv_id,
                title = %candidate.title,
                publisher = ?volume.publisher,
                rows_affected = rows,
                "cv_enrichment.refreshed"
            );
        }
        Err(e) => {
            tracing::warn!(
                target: "longbox_cv_enrichment",
                series_id = candidate.series_id,
                error = %e,
                "cv_enrichment.refresh_write_failed"
            );
        }
    }
}

/// **The load-bearing per-series procedure.** Two phases:
///
/// 1. **Outside transaction** (network + decisions). Pre-filter →
///    [`pick_volume`] → cv_id_collision pre-check via
///    `find_by_cv_id` → `fetch_volume` + `fetch_issues`. If any of
///    these refuses or errors, record the outcome with a single
///    UPDATE statement (no transaction needed for the bookkeeping
///    write since nothing else changed) and return.
/// 2. **Inside transaction** (atomic write). Open transaction,
///    `set_cv_id` (returns rows-affected — 0 means race-lost, real
///    outcome), per-issue
///    `upsert_by_series_id_and_number_with_cv_fields`, post-upsert
///    orphan SELECT (the order matters — synthesized rows that
///    just got matched-and-updated by number now have cv_issue_id
///    set and correctly drop out of the orphan filter), classify
///    matched vs partial_merge, write
///    `record_enrichment_outcome`, commit.
///
/// Crash-safety: cancellation during phase 1 leaves the series
/// shallow (clean retry next cycle); cancellation during phase 2
/// rolls back the entire transaction including the outcome write
/// (the attempt timestamp staying unset is the proof in the
/// mid-tx test).
async fn attempt_one(
    db: &Pool,
    bg: &BackgroundCvClient,
    config: &EnrichmentConfig,
    candidate: &ShallowEnrichmentCandidate,
) -> AttemptOutcome {
    let input = EnrichmentCandidateInput {
        catalog_title: candidate.title.clone(),
        catalog_start_year: candidate.start_year.map(|y| y as i32),
        catalog_issue_count: candidate.issue_count as u32,
        catalog_title_collision: candidate.catalog_title_collision,
    };

    // Phase 1.a: Pre-filter via the pure pick_volume against an
    // empty pool — the CollisionDisabled and "we haven't searched
    // yet" cases need different handling. To get CollisionDisabled
    // *without* burning a CV call, run pick_volume against an empty
    // pool first; if the result is CollisionDisabled, we know the
    // pre-filter would have fired regardless of CV's response.
    let pre_check = pick_volume(&input, &[], config.thresholds);
    if matches!(pre_check, PickOutcome::CollisionDisabled) {
        let outcome = AttemptOutcome::CollisionDisabled;
        record_outcome_standalone(db, candidate.series_id, &outcome).await;
        return outcome;
    }

    // Phase 1.b: CV search. Background-throttled (30s floor).
    let pool: Vec<SeriesSearchResult> = match bg.search_volumes(&candidate.title).await {
        Ok(p) => p,
        Err(e) => {
            let outcome = AttemptOutcome::Error {
                detail: format!("search_volumes: {e}"),
            };
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
    };

    // Phase 1.c: pick_volume against the real pool.
    let pick = pick_volume(&input, &pool, config.thresholds);
    let (cv_id, score) = match pick {
        PickOutcome::Matched { cv_id, score } => (cv_id, score),
        PickOutcome::NoResults => {
            let outcome = AttemptOutcome::NoResults;
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
        PickOutcome::LowConfidence { best_score } => {
            let outcome = AttemptOutcome::LowConfidence { best_score };
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
        PickOutcome::MultiMatch {
            best_score,
            second_score,
        } => {
            let outcome = AttemptOutcome::MultiMatch {
                best_score,
                second_score,
            };
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
        PickOutcome::YearMismatch => {
            let outcome = AttemptOutcome::YearMismatch;
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
        PickOutcome::CountMismatch => {
            let outcome = AttemptOutcome::CountMismatch;
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
        PickOutcome::CollisionDisabled => {
            // The pre-check above should have caught this. Defensive.
            let outcome = AttemptOutcome::CollisionDisabled;
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
    };

    // Phase 1.d: cv_id_collision pre-check. Before opening a
    // transaction (so the diagnostic carries the colliding series
    // id, not a unique-violation error).
    match series_repo::find_by_cv_id(db, cv_id).await {
        Ok(Some(other)) => {
            let outcome = AttemptOutcome::CvIdCollision {
                cv_id,
                other_series_id: other.id,
            };
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
        Ok(None) => {}
        Err(e) => {
            let outcome = AttemptOutcome::Error {
                detail: format!("find_by_cv_id: {e}"),
            };
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
    }

    // Phase 1.e: CV volume + issues fetch. Both calls
    // background-throttled by separate 30s slots.
    let volume: CvVolumeDetail = match bg.fetch_volume(cv_id).await {
        Ok(v) => v,
        Err(e) => {
            let outcome = AttemptOutcome::Error {
                detail: format!("fetch_volume: {e}"),
            };
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
    };
    let cv_issues: Vec<CvIssueDetail> = match bg.fetch_issues(cv_id).await {
        Ok(i) => i,
        Err(e) => {
            let outcome = AttemptOutcome::Error {
                detail: format!("fetch_issues: {e}"),
            };
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            return outcome;
        }
    };

    // Phase 2: atomic write. Single transaction; if anything
    // errors, the entire transaction rolls back including the
    // outcome write.
    match commit_merge(db, candidate.series_id, cv_id, score, &volume, cv_issues).await {
        Ok(outcome) => outcome,
        Err(e) => {
            let outcome = AttemptOutcome::Error {
                detail: format!("commit_merge: {e}"),
            };
            // The transaction rolled back; bookkeeping write is
            // standalone.
            record_outcome_standalone(db, candidate.series_id, &outcome).await;
            outcome
        }
    }
}

/// Phase-2 transaction. All writes atomic with each other:
/// `set_cv_id`, per-issue upserts, post-upsert orphan SELECT, and
/// the outcome record. Returns the worker-internal outcome (Matched
/// or PartialMerge or SetCvIdRaceLost).
async fn commit_merge(
    db: &Pool,
    series_id: i64,
    cv_id: i64,
    score: f64,
    volume: &CvVolumeDetail,
    cv_issues: Vec<CvIssueDetail>,
) -> Result<AttemptOutcome, longbox_db::DbError> {
    let mut tx = db.begin().await.map_err(longbox_db::DbError::from)?;

    // (a) Promote the series. rows_affected == 0 means another
    // writer claimed it first — real outcome, log explicitly.
    let rows = series_repo::set_cv_id(&mut *tx, series_id, cv_id).await?;
    if rows == 0 {
        // Race lost. Don't write outcome inside the transaction —
        // the predicate-failure IS the outcome, and the series's
        // existing state (now CV-linked by whoever raced) is the
        // truth. Roll back this tx (no-op for the empty
        // series-update; the merge wouldn't apply because we don't
        // have the right cv_id for the now-linked series) and
        // record the race outcome standalone.
        tx.rollback().await.map_err(longbox_db::DbError::from)?;
        let outcome = AttemptOutcome::SetCvIdRaceLost { cv_id };
        // Standalone outcome write — the bookkeeping is independent
        // of any merge we just refused to do.
        record_outcome_standalone(db, series_id, &outcome).await;
        return Ok(outcome);
    }

    // (a.1) Persist the descriptive volume detail (publisher,
    // description, cover_url) carried in the CvVolumeDetail. All
    // three would otherwise be discarded — the calendar's publisher
    // grouping (and the series detail view's description / cover)
    // depend on these. Inside the same transaction so they roll
    // back atomically with the rest of the merge.
    series_repo::update_series_volume_detail(
        &mut *tx,
        series_id,
        volume.publisher.as_deref(),
        volume.description.as_deref(),
        volume.cover_url.as_deref(),
    )
    .await?;

    // (b) Merge each CV issue via the row-id-preserving upsert.
    let mut cv_numbers: HashSet<String> = HashSet::with_capacity(cv_issues.len());
    for cv_issue in cv_issues {
        cv_numbers.insert(cv_issue.issue_number.clone());
        issue_repo::upsert_by_series_id_and_number_with_cv_fields(
            &mut *tx,
            NewIssue {
                series_id,
                cv_issue_id: Some(cv_issue.cv_issue_id),
                metron_issue_id: None,
                number: cv_issue.issue_number,
                title: cv_issue.name,
                cover_date: cv_issue.cover_date,
                summary: cv_issue.description,
                cover_url: cv_issue.cover_url,
            },
        )
        .await?;
    }

    // (c) Post-upsert orphan SELECT. Order is load-bearing — must
    // run AFTER the upserts so synthesized rows that just got
    // matched-and-updated by number now have cv_issue_id set and
    // correctly drop out of the cv_issue_id IS NULL filter. Pre-
    // upsert it would count every synthesized row as orphan.
    let orphan_numbers =
        series_repo::list_orphan_synthesized_numbers(&mut *tx, series_id).await?;

    // (d) Classify outcome.
    let outcome = if orphan_numbers.is_empty() {
        AttemptOutcome::Matched { cv_id, score }
    } else {
        AttemptOutcome::PartialMerge {
            cv_id,
            score,
            orphan_count: orphan_numbers.len(),
            orphan_numbers: orphan_numbers.clone(),
        }
    };

    // (e) Record outcome in the same transaction. The attempt
    // timestamp rolling back with everything else is the property
    // the crash-safety test verifies.
    let diagnostic = outcome.diagnostic();
    series_repo::record_enrichment_outcome(
        &mut *tx,
        series_id,
        outcome.as_db_str(),
        diagnostic.as_deref(),
    )
    .await?;

    tx.commit().await.map_err(longbox_db::DbError::from)?;
    Ok(outcome)
}

/// Record outcome standalone (no merge to roll back). Used for
/// refusals + errors + race-lost where no write needs to roll back
/// atomically with the outcome.
async fn record_outcome_standalone(db: &Pool, series_id: i64, outcome: &AttemptOutcome) {
    let diagnostic = outcome.diagnostic();
    if let Err(e) = series_repo::record_enrichment_outcome(
        db,
        series_id,
        outcome.as_db_str(),
        diagnostic.as_deref(),
    )
    .await
    {
        tracing::warn!(
            target: "longbox_cv_enrichment",
            series_id,
            outcome = outcome.as_db_str(),
            error = %e,
            "cv_enrichment.outcome_write_failed"
        );
    }
}

fn log_outcome(candidate: &ShallowEnrichmentCandidate, outcome: &AttemptOutcome) {
    use AttemptOutcome as O;
    match outcome {
        O::Matched { cv_id, score } => tracing::info!(
            target: "longbox_cv_enrichment",
            series_id = candidate.series_id,
            title = %candidate.title,
            cv_id,
            score,
            "cv_enrichment.matched"
        ),
        O::PartialMerge {
            cv_id,
            orphan_count,
            orphan_numbers,
            ..
        } => tracing::warn!(
            target: "longbox_cv_enrichment",
            series_id = candidate.series_id,
            title = %candidate.title,
            cv_id,
            orphan_count,
            orphan_numbers = ?orphan_numbers,
            "cv_enrichment.partial_merge"
        ),
        O::CvIdCollision {
            cv_id,
            other_series_id,
        } => tracing::warn!(
            target: "longbox_cv_enrichment",
            series_id = candidate.series_id,
            cv_id,
            other_series_id,
            "cv_enrichment.cv_id_collision"
        ),
        O::SetCvIdRaceLost { cv_id } => tracing::info!(
            target: "longbox_cv_enrichment",
            series_id = candidate.series_id,
            cv_id,
            "cv_enrichment.set_cv_id_race_lost"
        ),
        O::CollisionDisabled => tracing::debug!(
            target: "longbox_cv_enrichment",
            series_id = candidate.series_id,
            "cv_enrichment.collision_disabled"
        ),
        _ => tracing::debug!(
            target: "longbox_cv_enrichment",
            series_id = candidate.series_id,
            outcome = outcome.as_db_str(),
            "cv_enrichment.refused"
        ),
    }
}

/// Verify the three 6c.1 migration columns exist on `series` via
/// the canonical repo method. Returns the list of missing column
/// names on failure. Defense around the deferred migration-not-
/// applied-on-startup gremlin: converts silent no-op into loud
/// refusal at boot.
pub async fn assert_schema(db: &Pool) -> Result<(), Vec<String>> {
    match series_repo::enrichment_schema_check(db).await {
        Ok(missing) if missing.is_empty() => Ok(()),
        Ok(missing) => Err(missing),
        Err(e) => Err(vec![format!("schema_check_failed: {e}")]),
    }
}

