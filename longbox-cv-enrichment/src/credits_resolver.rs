//! Continuous low-priority credits resolver. Drains owned, CV-keyed issues
//! whose `credits_fetched = 0` by fetching their per-issue `person_credits`
//! on the shared `BackgroundCvClient` lane, which has its own budget apart
//! from interactive requests. Fire-and-forget; no settings gate.
use std::time::Duration;

use longbox_comicvine::CvError;
use longbox_db::{creator_repo, Pool};

use crate::background::BackgroundCvClient;

/// Idle re-check interval when there's no work (new subscribes show up here).
const IDLE_SLEEP: Duration = Duration::from_secs(300);
/// Issues per work-list batch.
const BATCH: i64 = 50;

/// A per-issue error that won't resolve on retry (mark the issue done with
/// zero credits so it stops churning the work-list). NotFound (404 / CV 101),
/// a malformed CV payload, and non-429 4xx are all permanent for one issue.
/// Auth / rate-limit / 5xx / network / timeout are transient (left for retry).
fn is_terminal(e: &CvError) -> bool {
    match e {
        CvError::NotFound => true,
        CvError::Malformed { .. } => true,
        CvError::Http { status, .. } => (400..500).contains(status) && *status != 429,
        _ => false,
    }
}

/// Spawn the resolver onto the tokio runtime. `bg` is the process-wide
/// background CV client (shared with the enrichment worker and the
/// catalog refresh).
pub fn spawn_credits_resolver(db: Pool, bg: BackgroundCvClient) {
    tokio::spawn(credits_loop(db, bg));
}

async fn credits_loop(db: Pool, bg: BackgroundCvClient) {
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
                // Permanent for this issue — mark done (zero credits), no churn.
                Err(e) if is_terminal(&e) => {
                    let _ = creator_repo::insert_issue_credits(&db, item.issue_id, &[]).await;
                }
                // Transient (auth / rate-limit / 5xx / network / timeout) — retry next pass.
                Err(e) => tracing::warn!(target: "longbox_credits",
                    cv_issue_id = item.cv_issue_id, error = %e, "credit fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_credits", processed, "credits resolver pass");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_terminal_classifies_correctly() {
        assert!(is_terminal(&CvError::NotFound));
        assert!(is_terminal(&CvError::Http {
            status: 404,
            body: String::new()
        }));
        assert!(is_terminal(&CvError::Http {
            status: 400,
            body: String::new()
        }));
        assert!(is_terminal(&CvError::Malformed {
            message: "bad".into(),
            raw_excerpt: None
        }));
        // 429 is rate-limit — transient, not terminal.
        assert!(!is_terminal(&CvError::Http {
            status: 429,
            body: String::new()
        }));
        // 5xx — transient.
        assert!(!is_terminal(&CvError::Http {
            status: 500,
            body: String::new()
        }));
        // Timeout — transient.
        assert!(!is_terminal(&CvError::Timeout));
    }
}
