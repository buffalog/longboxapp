//! Continuous Metron-linking resolver. Drains CV-linked series not yet checked
//! against Metron, matching each by its ComicVine id (`series/?cv_id=`), and
//! recording the outcome: matched -> `metron_id`; no match -> checked-only (so
//! it never re-queries). Fire-and-forget; Metron's own rate limiter throttles.
//! Populating `metron_id` activates the dormant series-finished enrichment.
use std::sync::Arc;
use std::time::Duration;

use longbox_db::{series_repo, Pool};
use longbox_metron::MetronClient;

/// Idle re-check interval when the work-list is empty.
const IDLE_SLEEP: Duration = Duration::from_secs(300);
/// Series per work-list batch.
const BATCH: i64 = 50;

/// Spawn the linker onto the tokio runtime. `metron` is the shared client.
pub fn spawn_metron_linker(db: Pool, metron: Arc<MetronClient>) {
    tokio::spawn(link_loop(db, metron));
}

async fn link_loop(db: Pool, metron: Arc<MetronClient>) {
    loop {
        let batch = match series_repo::list_metron_link_candidates(&db, BATCH).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "longbox_metron_link", error = %e, "candidate query failed");
                tokio::time::sleep(IDLE_SLEEP).await;
                continue;
            }
        };
        if batch.is_empty() {
            tokio::time::sleep(IDLE_SLEEP).await;
            continue;
        }
        let (mut linked, mut no_match) = (0usize, 0usize);
        for (series_id, cv_id) in &batch {
            match metron.fetch_series_by_cv_id(*cv_id).await {
                Ok(Some(sref)) => {
                    let mid = sref.metron_series_id.to_string();
                    match series_repo::mark_metron_link_checked(&db, *series_id, Some(&mid)).await {
                        Ok(_) => linked += 1,
                        Err(e) => tracing::warn!(target: "longbox_metron_link",
                            series_id, error = %e, "mark link failed"),
                    }
                }
                // Metron has no series for this CV id — mark checked (no match)
                // so it drops out of the work-list; do not churn.
                Ok(None) => {
                    let _ = series_repo::mark_metron_link_checked(&db, *series_id, None).await;
                    no_match += 1;
                }
                // Transient (rate-limit / network / http) — leave unchecked, retry.
                Err(e) => tracing::warn!(target: "longbox_metron_link",
                    cv_id, error = %e, "metron series fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_metron_link", linked, no_match, "metron link pass");
    }
}
