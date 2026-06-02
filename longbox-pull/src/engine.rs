//! The pull sweep — one pass over the pull list.
//!
//! A sweep runs two phases:
//!
//! 1. **Poll in-flight.** Every `submitted` `pull_attempt` is checked
//!    against the downloader. A failed grab transitions the attempt to
//!    `failed`; a download the downloader has lost track of is failed
//!    after [`UNKNOWN_POLL_LIMIT`] consecutive `Unknown` polls.
//! 2. **Submit new pulls.** For each active pull-list series, un-owned
//!    shipped issues that aren't already settled or parked are searched
//!    across the indexers; a match's NZB is handed to the downloader
//!    and recorded as a `submitted` attempt.
//!
//! No ComicVine: the sweep acts only on issues already in the catalog.
//! A subscribed series whose newly-*announced* issues have not been
//! refreshed into the catalog will not have them pulled — the user must
//! refresh that series. Phase A.8 Step 8's release calendar is the
//! intended discovery path; closing that gap is out of Step 6's scope.

use std::cmp::Ordering;

use longbox_core::{IssueNumber, ParsingPattern, PULL_INDEXER_MATCH_THRESHOLD};
use longbox_db::{
    downloader_config_repo, indexer_config_repo, issue_repo, parsing_pattern_repo,
    pull_attempt_repo, pull_list_repo, series_repo, settings_repo, webhook_config_repo,
    DownloaderConfigRow, IndexerConfigRow, IssueRow, NewPullAttempt, Pool, PullListRow, SeriesRow,
};
use longbox_downloader::{
    connect, AnyDownloader, DownloadHandle, DownloadStatus, Downloader, DownloaderAuth,
    DownloaderConfig,
};
use longbox_newznab::{FindOutcome, IndexerConfig, IndexerId, NewznabError};
use longbox_webhooks::WebhookEvent;

use crate::dispatch;
use crate::error::PullError;

/// Consecutive `Unknown` status polls tolerated before the engine
/// gives up on an in-flight download and fails the attempt. A flaky
/// downloader can briefly drop a job from both its queue and history;
/// one `Unknown` is not enough to conclude the grab is lost.
const UNKNOWN_POLL_LIMIT: i64 = 3;

/// Failed-attempt count at which an issue is parked — the pull engine
/// retries it no further. Mirrors the parking threshold baked into
/// `issue_repo::list_pull_candidates`'s SQL; reaching it is what a
/// `pull_failed` notification reports.
const RETRY_CAP: i64 = 3;

/// Tallies from one sweep — logged by the scheduler, asserted by tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepSummary {
    /// In-flight attempts polled for downloader status.
    pub polled: usize,
    /// In-flight attempts transitioned to `failed` by polling — a grab
    /// failure or a lost-track timeout.
    pub grab_failed: usize,
    /// New NZBs handed to the downloader.
    pub submitted: usize,
    /// Candidate issues with no indexer match — retried next sweep.
    pub no_match: usize,
    /// Submission failures — the downloader rejected or was unreachable.
    pub submission_failed: usize,
    /// Series-title mismatches (Bug 3) — indexer returned releases but
    /// none survived the pre-grab similarity filter. Recorded as
    /// `pull_attempts.status='mismatched'`; counts toward retry_count
    /// like a failure.
    pub series_mismatched: usize,
    /// Candidate issues skipped because every indexer errored.
    pub indexer_errors: usize,
}

/// Per-sweep context: the rebuilt downloader, the enabled indexers,
/// the parsing patterns, and the pre-grab similarity threshold. Loaded
/// once per sweep so a settings change tunes the next sweep without
/// restart, and shared between `sweep` (all series) and
/// `sweep_single_series` (one series) so the on-demand search path
/// gets the exact same gating.
struct SweepContext {
    downloader: AnyDownloader,
    indexers: Vec<IndexerConfig>,
    patterns: Vec<ParsingPattern>,
    similarity_threshold: f64,
    /// Comma-split, trimmed, non-empty entries from the
    /// `pull_exclusion_keywords` setting. The pre-grab filter drops
    /// any release whose normalized title contains one of these as
    /// a substring — used to keep digital-only formats (Marvel
    /// Infinity Comic, DC Infinite Comic) out of the library.
    exclusion_keywords: Vec<String>,
}

/// Reasons a sweep can't run. `NoDownloader` and `DownloaderDisabled`
/// short-circuit before Phase 1 (without a downloader the engine can't
/// even poll in-flight attempts). `NoIndexers` blocks Phase 2 only;
/// the all-series caller logs it and returns the polling result
/// anyway, the single-series caller propagates it as a no-op summary.
enum SweepGate {
    NoDownloader,
    DownloaderDisabled,
    NoIndexers,
}

/// Load everything Phase 2 needs. `Ok(Ok(ctx))` means full sweep is
/// possible. `Ok(Err(gate))` means a precondition failed and the
/// caller should log + bail. `Err(_)` is a real DB failure.
async fn load_sweep_context(db: &Pool) -> Result<Result<SweepContext, SweepGate>, PullError> {
    let Some(downloader_row) = downloader_config_repo::get(db).await? else {
        return Ok(Err(SweepGate::NoDownloader));
    };
    if !downloader_row.enabled {
        return Ok(Err(SweepGate::DownloaderDisabled));
    }
    let downloader = build_downloader(downloader_row);

    let indexer_rows = indexer_config_repo::list_enabled(db).await?;
    if indexer_rows.is_empty() {
        return Ok(Err(SweepGate::NoIndexers));
    }
    let indexers: Vec<IndexerConfig> = indexer_rows.into_iter().map(to_indexer_config).collect();

    let patterns = load_patterns(db).await?;
    let similarity_threshold = settings_repo::get_or_default(
        db,
        settings_repo::KEY_PULL_INDEXER_MATCH_THRESHOLD,
        PULL_INDEXER_MATCH_THRESHOLD,
    )
    .await?;
    let exclusion_keywords = load_exclusion_keywords(db).await?;

    Ok(Ok(SweepContext {
        downloader,
        indexers,
        patterns,
        similarity_threshold,
        exclusion_keywords,
    }))
}

/// Read `pull_exclusion_keywords` from settings, split on `,`, trim
/// whitespace, drop empty entries. Stored as a CSV string in one row
/// so editing via plain SQL stays ergonomic (no JSON array escapes).
/// On missing-row / read-error, returns the conservative default — no
/// keywords, no exclusions, the indexer's full result pool passes the
/// pre-grab filter as before.
async fn load_exclusion_keywords(db: &Pool) -> Result<Vec<String>, PullError> {
    let raw: String = settings_repo::get_or_default(
        db,
        settings_repo::KEY_PULL_EXCLUSION_KEYWORDS,
        String::new(),
    )
    .await?;
    Ok(raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// Run one pull sweep against the catalog and the configured services.
pub async fn sweep(db: &Pool) -> Result<SweepSummary, PullError> {
    let mut summary = SweepSummary::default();

    // Phase 1 needs only the downloader; load just enough to drive it
    // without paying the Phase 2 load costs (indexers, patterns,
    // threshold) up front.
    let Some(downloader_row) = downloader_config_repo::get(db).await? else {
        tracing::info!(target: "longbox_pull", "pull.sweep_skipped (no downloader configured)");
        return Ok(summary);
    };
    if !downloader_row.enabled {
        tracing::info!(target: "longbox_pull", "pull.sweep_skipped (downloader disabled)");
        return Ok(summary);
    }
    let downloader = build_downloader(downloader_row);

    // Phase 1: poll in-flight attempts (needs only the downloader).
    poll_in_flight(db, &downloader, &mut summary).await?;

    // Phase 2: submit new pulls. Needs at least one enabled indexer.
    let indexer_rows = indexer_config_repo::list_enabled(db).await?;
    if indexer_rows.is_empty() {
        tracing::info!(target: "longbox_pull", "pull.sweep_no_submit (no enabled indexers)");
        return Ok(summary);
    }
    let indexers: Vec<IndexerConfig> = indexer_rows.into_iter().map(to_indexer_config).collect();

    let patterns = load_patterns(db).await?;
    let similarity_threshold = settings_repo::get_or_default(
        db,
        settings_repo::KEY_PULL_INDEXER_MATCH_THRESHOLD,
        PULL_INDEXER_MATCH_THRESHOLD,
    )
    .await?;
    let exclusion_keywords = load_exclusion_keywords(db).await?;

    for entry in pull_list_repo::list_active(db).await? {
        sweep_series(
            db,
            &entry,
            &indexers,
            &downloader,
            &patterns,
            similarity_threshold,
            &exclusion_keywords,
            &mut summary,
        )
        .await?;
    }
    Ok(summary)
}

/// Run Phase 2 (issue enumeration + indexer submission) for exactly
/// one subscribed series. Skips Phase 1 — in-flight polling belongs
/// to the daily scheduler. Returns `NotOnPullList` when the series
/// has no pull-list entry.
///
/// Documented race: if the daily scheduler is mid-sweep on the same
/// series at the moment this fires, both could query indexers for the
/// same candidates and try to submit duplicate `pull_attempts`. The
/// unique key on `(series_id, issue_id)` absorbs the collision — the
/// second INSERT errors out and the per-issue code path logs and moves
/// on. Window is bounded (each sweep_series call is short); cost on
/// collision is one wasted indexer query.
pub async fn sweep_single_series(
    db: &Pool,
    series_id: i64,
) -> Result<SweepSummary, PullError> {
    let mut summary = SweepSummary::default();
    let entry =
        pull_list_repo::get(db, series_id)
            .await?
            .ok_or(PullError::NotOnPullList { series_id })?;
    let ctx = match load_sweep_context(db).await? {
        Ok(ctx) => ctx,
        Err(SweepGate::NoDownloader) => {
            tracing::info!(target: "longbox_pull", series_id, "pull.search_skipped (no downloader configured)");
            return Ok(summary);
        }
        Err(SweepGate::DownloaderDisabled) => {
            tracing::info!(target: "longbox_pull", series_id, "pull.search_skipped (downloader disabled)");
            return Ok(summary);
        }
        Err(SweepGate::NoIndexers) => {
            tracing::info!(target: "longbox_pull", series_id, "pull.search_skipped (no enabled indexers)");
            return Ok(summary);
        }
    };
    sweep_series(
        db,
        &entry,
        &ctx.indexers,
        &ctx.downloader,
        &ctx.patterns,
        ctx.similarity_threshold,
        &ctx.exclusion_keywords,
        &mut summary,
    )
    .await?;
    Ok(summary)
}

/// Load active parsing patterns from the catalog, mapping the row form
/// into `longbox-core`'s `ParsingPattern`. Used by Bug 3 to feed release-
/// title parsing inside the newznab pre-grab filter — same parser the
/// scanner uses on disk filenames, so any pattern hardening propagates.
async fn load_patterns(db: &Pool) -> Result<Vec<ParsingPattern>, PullError> {
    Ok(parsing_pattern_repo::list_enabled(db)
        .await?
        .into_iter()
        .map(|r| ParsingPattern {
            id: r.id,
            name: r.name,
            pattern: r.pattern,
            priority: i32::try_from(r.priority).unwrap_or(i32::MAX),
            enabled: r.enabled,
        })
        .collect())
}

/// Phase 1 — check every `submitted` attempt against the downloader.
async fn poll_in_flight(
    db: &Pool,
    downloader: &AnyDownloader,
    summary: &mut SweepSummary,
) -> Result<(), PullError> {
    for attempt in pull_attempt_repo::list_submitted(db).await? {
        // A `submitted` row always carries a handle (the engine sets it
        // on the successful submit); skip defensively if it somehow
        // does not.
        let Some(handle) = attempt.download_handle.clone() else {
            continue;
        };
        summary.polled += 1;
        match downloader.status(&DownloadHandle(handle)).await {
            Ok(DownloadStatus::Completed) => {
                // The file is landing in the watch folder; Phase B
                // catches it and flips the attempt to `grabbed`.
            }
            Ok(DownloadStatus::Queued | DownloadStatus::Downloading) => {
                // Still in progress. A known status resets the
                // consecutive-`Unknown` counter.
                if attempt.unknown_polls > 0 {
                    pull_attempt_repo::reset_unknown_polls(db, attempt.id).await?;
                }
            }
            Ok(DownloadStatus::Failed(reason)) => {
                let msg = format!("download failed: {reason}");
                pull_attempt_repo::record_failure(db, attempt.id, &msg).await?;
                summary.grab_failed += 1;
                maybe_fire_pull_failed(db, attempt.series_id, attempt.issue_id, &msg).await;
            }
            Ok(DownloadStatus::Unknown) => {
                if attempt.unknown_polls + 1 >= UNKNOWN_POLL_LIMIT {
                    let msg = "lost track of download — the downloader no longer reports this job";
                    pull_attempt_repo::record_failure(db, attempt.id, msg).await?;
                    summary.grab_failed += 1;
                    maybe_fire_pull_failed(db, attempt.series_id, attempt.issue_id, msg).await;
                } else {
                    pull_attempt_repo::bump_unknown_polls(db, attempt.id).await?;
                }
            }
            Err(e) => {
                // Transient downloader error — leave the attempt
                // `submitted` and retry the poll next sweep.
                tracing::warn!(
                    target: "longbox_pull",
                    attempt_id = attempt.id,
                    error = %e,
                    "pull.poll_failed"
                );
            }
        }
    }
    Ok(())
}

/// Phase 2, per series — enumerate candidates and attempt each.
#[allow(clippy::too_many_arguments)] // sweep-context bag — mirrors
                                     // attempt_pull_for_candidate's
                                     // existing too-many-args waiver.
async fn sweep_series(
    db: &Pool,
    entry: &PullListRow,
    indexers: &[IndexerConfig],
    downloader: &AnyDownloader,
    patterns: &[ParsingPattern],
    similarity_threshold: f64,
    exclusion_keywords: &[String],
    summary: &mut SweepSummary,
) -> Result<(), PullError> {
    let Some(series) = series_repo::find_by_id(db, entry.series_id).await? else {
        // Series row gone — the pull_list FK CASCADE will clean up.
        return Ok(());
    };
    let candidates = issue_repo::list_pull_candidates(db, entry.series_id).await?;
    let candidates = apply_start_floor(candidates, entry.start_issue.as_deref());

    let mut any_submitted = false;
    let mut any_failed = false;

    for issue in candidates {
        match attempt_pull_for_candidate(
            db,
            &series,
            &issue,
            indexers,
            downloader,
            patterns,
            similarity_threshold,
            exclusion_keywords,
            summary,
        )
        .await?
        {
            AttemptOutcome::Submitted => any_submitted = true,
            AttemptOutcome::Failed => any_failed = true,
            AttemptOutcome::NoMatch | AttemptOutcome::IndexerErrors => {}
        }
    }

    // Stamp the series' pull-list state. `last_successful_pull_at`
    // tracks the last successful *submission* (the grab is tracked
    // per-attempt). A sweep that only no-matched is not a failure —
    // leave the counters untouched.
    if any_submitted {
        pull_list_repo::mark_attempt_succeeded(db, entry.series_id).await?;
    } else if any_failed {
        pull_list_repo::mark_attempt_failed(db, entry.series_id).await?;
    }
    Ok(())
}

/// What `attempt_pull_for_candidate` did with one issue. Drives
/// `sweep_series`'s pull-list-bookkeeping branch at the end of a
/// sweep; `sweep_single_issue` ignores it (the series might not be
/// subscribed at all, so updating its pull-list timestamps would be
/// nonsensical).
enum AttemptOutcome {
    Submitted,
    Failed,
    NoMatch,
    IndexerErrors,
}

/// Run the full match-then-submit dance for ONE issue: search the
/// indexers (excluding prior grab-failed release ids), submit a hit to
/// the downloader, record the attempt row. The `summary` counters are
/// updated in place; the return value distinguishes submitted /
/// failed / no-match / all-indexers-down so callers can decide on
/// pull-list bookkeeping.
///
/// Extracted from `sweep_series`'s loop so the on-demand single-issue
/// search path can reuse it without duplicating the indexer-and-
/// downloader-and-attempt flow.
#[allow(clippy::too_many_arguments)] // sweep-context bag — extracting a struct
                                     // for two call sites is not worth the
                                     // indirection.
async fn attempt_pull_for_candidate(
    db: &Pool,
    series: &SeriesRow,
    issue: &IssueRow,
    indexers: &[IndexerConfig],
    downloader: &AnyDownloader,
    patterns: &[ParsingPattern],
    similarity_threshold: f64,
    exclusion_keywords: &[String],
    summary: &mut SweepSummary,
) -> Result<AttemptOutcome, PullError> {
    let series_id = series.id;
    // No year-hint for pull searches. The previous attempts at this
    // (start_year, then cover_date) both silently dropped valid
    // hits: CV's recorded cover_date is the cover-month of the
    // physical issue, but NZB titles carry the SCAN year — and
    // those routinely differ (off-by-one solicitation windows,
    // late physical scans of older digital releases, scanner-group
    // re-rips of back issues). The year-gate's strict equality
    // turns each disagreement into a silent rejection. Since the
    // pull engine already knows which series + issue it's pulling
    // for, the series-title similarity filter is doing all the
    // wrong-volume disambiguation work; year here is at best
    // redundant and at worst correctness-destroying.
    let year_hint: Option<i32> = None;

    let prior = pull_attempt_repo::list_for_issue(db, series_id, issue.id).await?;
    // Retry-exclusion: skip releases already tried. Only a
    // *grab*-failed attempt carries a release_id — a submission
    // failure records none, so its release is retried as-is (the
    // downloader, not the NZB, was the suspect).
    let exclude: Vec<String> = prior.iter().filter_map(|a| a.release_id.clone()).collect();
    // Bug 3: failure-class for retry-count budget covers both
    // 'failed' and 'mismatched'; the candidate query parks the
    // issue once any row reaches retry_count >= 3 regardless of
    // which kind it is.
    let failed_count = prior
        .iter()
        .filter(|a| matches!(a.status.as_str(), "failed" | "mismatched"))
        .count() as i64;

    match longbox_newznab::find_release_excluding_filtered(
        indexers,
        &series.title,
        &issue.number,
        year_hint,
        &exclude,
        patterns,
        similarity_threshold,
        exclusion_keywords,
    )
    .await
    {
        Ok(FindOutcome::Match {
            indexer: indexer_id,
            release,
        }) => {
            let name = format!("{} {}", series.title, issue.number);
            match downloader.submit(&release.nzb_url, &name).await {
                Ok(handle) => {
                    pull_attempt_repo::insert(
                        db,
                        NewPullAttempt {
                            series_id,
                            issue_id: issue.id,
                            indexer_id: Some(indexer_id.0),
                            release_id: Some(release.guid),
                            status: "submitted".into(),
                            error_message: None,
                            retry_count: failed_count,
                            download_handle: Some(handle.0),
                        },
                    )
                    .await?;
                    summary.submitted += 1;
                    Ok(AttemptOutcome::Submitted)
                }
                Err(e) => {
                    // Submission failure — record a failed attempt
                    // with no release_id so the same release is
                    // retried next sweep.
                    let reason = format!("submit failed: {e}");
                    pull_attempt_repo::insert(
                        db,
                        NewPullAttempt {
                            series_id,
                            issue_id: issue.id,
                            indexer_id: Some(indexer_id.0),
                            release_id: None,
                            status: "failed".into(),
                            error_message: Some(reason.clone()),
                            retry_count: failed_count + 1,
                            download_handle: None,
                        },
                    )
                    .await?;
                    summary.submission_failed += 1;
                    maybe_fire_pull_failed(db, series_id, issue.id, &reason).await;
                    Ok(AttemptOutcome::Failed)
                }
            }
        }
        Ok(FindOutcome::Mismatch {
            indexer: indexer_id,
            diagnostic,
        }) => {
            // Bug 3: indexer returned releases but none survived the
            // series-title filter. Record as 'mismatched' (counts
            // toward retry_count like a submission failure) and let
            // the existing cap+webhook path handle the surfacing.
            let reason = diagnostic.into_error_message(&series.title, similarity_threshold);
            pull_attempt_repo::record_mismatch(
                db,
                series_id,
                issue.id,
                Some(indexer_id.0),
                &reason,
                failed_count,
            )
            .await?;
            summary.series_mismatched += 1;
            maybe_fire_pull_failed(db, series_id, issue.id, &reason).await;
            Ok(AttemptOutcome::Failed)
        }
        Ok(FindOutcome::NoMatch) => {
            // No indexer match — no attempt row; retried next sweep.
            summary.no_match += 1;
            Ok(AttemptOutcome::NoMatch)
        }
        Err(NewznabError::AllIndexersFailed(_)) => {
            // Every indexer errored — an infrastructure problem, not
            // an issue-specific one. No attempt row: parking the
            // issue for an indexer outage would be wrong. Retried
            // next sweep.
            tracing::warn!(
                target: "longbox_pull",
                series_id,
                issue_id = issue.id,
                "pull.all_indexers_failed"
            );
            summary.indexer_errors += 1;
            Ok(AttemptOutcome::IndexerErrors)
        }
    }
}

/// On-demand search for ONE specific issue. The series does NOT need
/// to be on the pull list — this is the "I found a gap in my
/// collection, fill it now" path off the series detail page.
///
/// Bypasses two of the standard sweep gates: the pull-list
/// `start_issue` floor (the user is targeting a specific issue
/// explicitly) and the `retry_count >= 3` parking (the user is
/// overriding a prior give-up). Still respects the in-flight gate
/// — if a `pending` / `submitted` / `grabbed` attempt already exists
/// for this issue, skip silently with a log line. Without that guard
/// a second click on Search would create a duplicate pull_attempt and
/// hand the SAME NZB to the downloader twice (pull_attempts has no
/// unique key on `(series_id, issue_id)` — multiple rows are expected
/// for retry tracking).
///
/// Errors map to typed `PullError` variants (`SeriesNotFound`,
/// `IssueNotFound`, `IssueSeriesMismatch`) so the web route can
/// surface clean 404s. Downloader / indexer preconditions silently
/// no-op via `load_sweep_context`'s gates with a log line, same as
/// `sweep_single_series`.
pub async fn sweep_single_issue(
    db: &Pool,
    series_id: i64,
    issue_id: i64,
) -> Result<SweepSummary, PullError> {
    let mut summary = SweepSummary::default();
    let series = series_repo::find_by_id(db, series_id)
        .await?
        .ok_or(PullError::SeriesNotFound { series_id })?;
    let issue = issue_repo::find_by_id(db, issue_id)
        .await?
        .ok_or(PullError::IssueNotFound { issue_id })?;
    // Defense against URL tampering: the issue must actually belong to
    // the series in the path.
    if issue.series_id != series_id {
        return Err(PullError::IssueSeriesMismatch {
            series_id,
            issue_id,
            actual_series_id: issue.series_id,
        });
    }

    // In-flight guard. Replaces the unique-key absorption I had wrongly
    // assumed existed at the schema level. `pull_attempts` carries
    // multiple rows per (series_id, issue_id) by design (retry history),
    // so a duplicate INSERT here would create a real duplicate submit.
    let prior = pull_attempt_repo::list_for_issue(db, series_id, issue_id).await?;
    if prior
        .iter()
        .any(|a| matches!(a.status.as_str(), "pending" | "submitted" | "grabbed"))
    {
        tracing::info!(
            target: "longbox_pull",
            series_id,
            issue_id,
            "pull.search_skipped (an in-flight attempt already exists)"
        );
        return Ok(summary);
    }

    let ctx = match load_sweep_context(db).await? {
        Ok(ctx) => ctx,
        Err(SweepGate::NoDownloader) => {
            tracing::info!(target: "longbox_pull", series_id, issue_id, "pull.search_skipped (no downloader configured)");
            return Ok(summary);
        }
        Err(SweepGate::DownloaderDisabled) => {
            tracing::info!(target: "longbox_pull", series_id, issue_id, "pull.search_skipped (downloader disabled)");
            return Ok(summary);
        }
        Err(SweepGate::NoIndexers) => {
            tracing::info!(target: "longbox_pull", series_id, issue_id, "pull.search_skipped (no enabled indexers)");
            return Ok(summary);
        }
    };

    // Direct call to the per-candidate function — no list_pull_candidates
    // gating, no start_floor. The user explicitly asked for this issue.
    attempt_pull_for_candidate(
        db,
        &series,
        &issue,
        &ctx.indexers,
        &ctx.downloader,
        &ctx.patterns,
        ctx.similarity_threshold,
        &ctx.exclusion_keywords,
        &mut summary,
    )
    .await?;
    Ok(summary)
}

/// Fire a `pull_failed` webhook event when this issue's failure-class
/// attempt count has reached [`RETRY_CAP`] — the point at which the
/// engine parks it and retries no further. A no-op below the cap.
/// Delivery is spawned by [`dispatch::dispatch`], so this never blocks
/// the sweep on webhook HTTP. Bug 3 widened the count to include
/// `'mismatched'` so series-mismatches cap-cross the same way submission
/// and grab failures do.
async fn maybe_fire_pull_failed(db: &Pool, series_id: i64, issue_id: i64, reason: &str) {
    let failed = match pull_attempt_repo::list_for_issue(db, series_id, issue_id).await {
        Ok(attempts) => attempts
            .iter()
            .filter(|a| matches!(a.status.as_str(), "failed" | "mismatched"))
            .count() as i64,
        Err(e) => {
            tracing::warn!(target: "longbox_pull", error = %e, "pull.failed_event_check_failed");
            return;
        }
    };
    if failed < RETRY_CAP {
        return;
    }
    let title = series_repo::find_by_id(db, series_id)
        .await
        .ok()
        .flatten()
        .map(|s| s.title)
        .unwrap_or_else(|| format!("series {series_id}"));
    dispatch::dispatch(
        db.clone(),
        webhook_config_repo::EVENT_PULL_FAILED,
        WebhookEvent {
            event: "pull_failed".into(),
            message: format!("Pull failed permanently: {title} — {reason}"),
        },
    );
}

/// Drop candidate issues below the pull entry's `start_issue` floor.
/// Uses natural issue-number order ("10" sorts above "2") — not
/// expressible in the candidate SQL, so applied here.
fn apply_start_floor(candidates: Vec<IssueRow>, start_issue: Option<&str>) -> Vec<IssueRow> {
    let Some(floor) = start_issue else {
        return candidates;
    };
    let floor = IssueNumber::new(floor.to_owned());
    candidates
        .into_iter()
        .filter(|i| {
            IssueNumber::natural_cmp(&IssueNumber::new(i.number.clone()), &floor) != Ordering::Less
        })
        .collect()
}

/// Map a DB indexer row into the `longbox-newznab` client input.
fn to_indexer_config(row: IndexerConfigRow) -> IndexerConfig {
    IndexerConfig {
        id: IndexerId(row.id),
        name: row.name,
        base_url: row.base_url,
        api_key: row.api_key,
        priority: row.priority as i32,
        maxage_days: row.maxage_days.max(0) as u32,
    }
}

/// Build the active downloader client from its DB config row. The
/// `downloader_config.kind` CHECK constraint guarantees `sab`/`nzbget`.
fn build_downloader(row: DownloaderConfigRow) -> AnyDownloader {
    let auth = match row.kind.as_str() {
        "nzbget" => DownloaderAuth::Basic {
            username: row.username.unwrap_or_default(),
            password: row.secret,
        },
        _ => DownloaderAuth::ApiKey(row.secret),
    };
    connect(&DownloaderConfig {
        base_url: row.base_url,
        auth,
        category: row.category,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn issue(number: &str) -> IssueRow {
        IssueRow {
            id: 0,
            series_id: 1,
            cv_issue_id: None,
            metron_issue_id: None,
            number: number.to_owned(),
            title: None,
            cover_date: None,
            summary: None,
            cover_url: None,
            created_at: datetime!(2026-01-01 0:00),
            updated_at: datetime!(2026-01-01 0:00),
        }
    }

    #[test]
    fn start_floor_none_keeps_everything() {
        let kept = apply_start_floor(vec![issue("1"), issue("2")], None);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn start_floor_keeps_at_or_above_in_natural_order() {
        let kept = apply_start_floor(
            vec![issue("1"), issue("2"), issue("9"), issue("10")],
            Some("2"),
        );
        // "1" drops; "2", "9", "10" stay — "10" is natural-greater
        // than the "2" floor (lexical order would wrongly drop it).
        let nums: Vec<&str> = kept.iter().map(|i| i.number.as_str()).collect();
        assert_eq!(nums, ["2", "9", "10"]);
    }
}
