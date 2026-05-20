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

use longbox_core::IssueNumber;
use longbox_db::{
    downloader_config_repo, indexer_config_repo, issue_repo, pull_attempt_repo, pull_list_repo,
    series_repo, DownloaderConfigRow, IndexerConfigRow, IssueRow, NewPullAttempt, Pool,
    PullListRow,
};
use longbox_downloader::{
    connect, AnyDownloader, DownloadHandle, DownloadStatus, Downloader, DownloaderAuth,
    DownloaderConfig,
};
use longbox_newznab::{IndexerConfig, IndexerId, NewznabError};

use crate::error::PullError;

/// Consecutive `Unknown` status polls tolerated before the engine
/// gives up on an in-flight download and fails the attempt. A flaky
/// downloader can briefly drop a job from both its queue and history;
/// one `Unknown` is not enough to conclude the grab is lost.
const UNKNOWN_POLL_LIMIT: i64 = 3;

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
    /// Candidate issues skipped because every indexer errored.
    pub indexer_errors: usize,
}

/// Run one pull sweep against the catalog and the configured services.
pub async fn sweep(db: &Pool) -> Result<SweepSummary, PullError> {
    let mut summary = SweepSummary::default();

    // The downloader is required for both phases — no downloader, no
    // sweep.
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

    for entry in pull_list_repo::list_active(db).await? {
        sweep_series(db, &entry, &indexers, &downloader, &mut summary).await?;
    }
    Ok(summary)
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
                pull_attempt_repo::record_failure(
                    db,
                    attempt.id,
                    &format!("download failed: {reason}"),
                )
                .await?;
                summary.grab_failed += 1;
            }
            Ok(DownloadStatus::Unknown) => {
                if attempt.unknown_polls + 1 >= UNKNOWN_POLL_LIMIT {
                    pull_attempt_repo::record_failure(
                        db,
                        attempt.id,
                        "lost track of download — the downloader no longer reports this job",
                    )
                    .await?;
                    summary.grab_failed += 1;
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
async fn sweep_series(
    db: &Pool,
    entry: &PullListRow,
    indexers: &[IndexerConfig],
    downloader: &AnyDownloader,
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
        let prior = pull_attempt_repo::list_for_issue(db, entry.series_id, issue.id).await?;
        // Retry-exclusion: skip releases already tried. Only a
        // *grab*-failed attempt carries a release_id — a submission
        // failure records none, so its release is retried as-is (the
        // downloader, not the NZB, was the suspect).
        let exclude: Vec<String> = prior.iter().filter_map(|a| a.release_id.clone()).collect();
        // The new row's retry_count is the cumulative failed count; the
        // candidate query parks the issue once any row reaches 3.
        let failed_count = prior.iter().filter(|a| a.status == "failed").count() as i64;

        match longbox_newznab::find_release_excluding(
            indexers,
            &series.title,
            &issue.number,
            None,
            &exclude,
        )
        .await
        {
            Ok(Some((indexer_id, release))) => {
                let name = format!("{} {}", series.title, issue.number);
                match downloader.submit(&release.nzb_url, &name).await {
                    Ok(handle) => {
                        pull_attempt_repo::insert(
                            db,
                            NewPullAttempt {
                                series_id: entry.series_id,
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
                        any_submitted = true;
                    }
                    Err(e) => {
                        // Submission failure — record a failed attempt
                        // with no release_id so the same release is
                        // retried next sweep.
                        pull_attempt_repo::insert(
                            db,
                            NewPullAttempt {
                                series_id: entry.series_id,
                                issue_id: issue.id,
                                indexer_id: Some(indexer_id.0),
                                release_id: None,
                                status: "failed".into(),
                                error_message: Some(format!("submit failed: {e}")),
                                retry_count: failed_count + 1,
                                download_handle: None,
                            },
                        )
                        .await?;
                        summary.submission_failed += 1;
                        any_failed = true;
                    }
                }
            }
            Ok(None) => {
                // No indexer match — no attempt row; retried next sweep.
                summary.no_match += 1;
            }
            Err(NewznabError::AllIndexersFailed(_)) => {
                // Every indexer errored — an infrastructure problem, not
                // an issue-specific one. No attempt row: parking the
                // issue for an indexer outage would be wrong. Retried
                // next sweep.
                tracing::warn!(
                    target: "longbox_pull",
                    series_id = entry.series_id,
                    issue_id = issue.id,
                    "pull.all_indexers_failed"
                );
                summary.indexer_errors += 1;
            }
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
