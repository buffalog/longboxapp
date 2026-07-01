//! Continuous low-priority credits resolver. Drains owned, CV-keyed issues
//! whose `credits_fetched = 0` by fetching their per-issue `person_credits`
//! behind the shared `BackgroundCvClient` throttle (~120 req/h, leaving the
//! interactive 180/h budget headroom). Fire-and-forget; no settings gate.
use std::sync::Arc;
use std::time::Duration;

use longbox_comicvine::{ComicVineClient, CvError};
use longbox_db::{creator_repo, Pool};

use crate::background::BackgroundCvClient;

/// Idle re-check interval when there's no work (new subscribes show up here).
const IDLE_SLEEP: Duration = Duration::from_secs(300);
/// Background call spacing (matches the enrichment worker default).
const REQUEST_INTERVAL: Duration = Duration::from_secs(30);
/// Issues per work-list batch.
const BATCH: i64 = 50;

/// Spawn the resolver onto the tokio runtime. `inner_cv` is the shared
/// 180/h CV client (same Arc the enrichment worker uses).
pub fn spawn_credits_resolver(db: Pool, inner_cv: Arc<ComicVineClient>) {
    tokio::spawn(credits_loop(db, inner_cv));
}

async fn credits_loop(db: Pool, inner_cv: Arc<ComicVineClient>) {
    let bg = BackgroundCvClient::new(inner_cv, REQUEST_INTERVAL);
    loop {
        let batch = match creator_repo::list_issues_needing_credits(&db, BATCH).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "longbox_credits", error = %e, "work-list query failed");
                tokio::time::sleep(IDLE_SLEEP).await;
                continue;
            }
        };
        if batch.is_empty() {
            tokio::time::sleep(IDLE_SLEEP).await;
            continue;
        }
        let mut processed = 0usize;
        for item in &batch {
            match bg.fetch_issue_credits(item.cv_issue_id).await {
                Ok(credits) => {
                    match creator_repo::insert_issue_credits(&db, item.issue_id, &credits).await {
                        Ok(()) => processed += 1,
                        Err(e) => tracing::warn!(target: "longbox_credits",
                            issue_id = item.issue_id, error = %e, "persist failed"),
                    }
                }
                // CV doesn't have the issue — mark done (zero credits) so it
                // doesn't churn the work-list forever.
                Err(CvError::NotFound) => {
                    let _ = creator_repo::insert_issue_credits(&db, item.issue_id, &[]).await;
                }
                // Transient (rate-limit / network / 5xx) — leave for retry.
                Err(e) => tracing::warn!(target: "longbox_credits",
                    cv_issue_id = item.cv_issue_id, error = %e, "credit fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_credits", processed, "credits resolver pass");
    }
}
