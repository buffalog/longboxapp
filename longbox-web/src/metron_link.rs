//! Continuous Metron-linking resolver. Drains CV-linked series not yet checked
//! against Metron, matching each by its ComicVine id (`series/?cv_id=`), and
//! recording the outcome: matched -> `metron_id`; no match -> checked-only (so
//! it never re-queries). Fire-and-forget; Metron's own rate limiter throttles.
//! Populating `metron_id` activates the dormant series-finished enrichment.
use std::sync::Arc;
use std::time::Duration;

use longbox_core::issue::IssueNumber;
use longbox_db::{issue_repo, series_repo, Pool};
use longbox_metron::{MetronClient, MetronError, MetronIssueRef};

/// Idle re-check interval when the work-list is empty.
const IDLE_SLEEP: Duration = Duration::from_secs(300);
/// Series per work-list batch.
const BATCH: i64 = 50;

/// A Metron error that won't resolve on retry for THIS series — mark it checked
/// (no link) so it can't pin the work-list. NotFound, a malformed payload, and
/// non-429 4xx are permanent per-request; Network/Timeout/RateLimited/Auth/5xx
/// are transient (left unchecked to retry). Mirrors credits_resolver::is_terminal.
fn is_terminal(e: &MetronError) -> bool {
    match e {
        MetronError::NotFound => true,
        MetronError::Malformed { .. } => true,
        MetronError::Http { status, .. } => (400..500).contains(status) && *status != 429,
        _ => false,
    }
}

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
        let (mut linked, mut checked_no_link) = (0usize, 0usize);
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
                    checked_no_link += 1;
                }
                // Permanent per-series error — mark checked (no link) so it can't pin
                // the work-list. NotFound / Malformed / non-429 4xx won't change on retry.
                Err(e) if is_terminal(&e) => {
                    tracing::debug!(target: "longbox_metron_link",
                        series_id, cv_id, error = %e, "terminal metron error; marking checked");
                    let _ = series_repo::mark_metron_link_checked(&db, *series_id, None).await;
                    checked_no_link += 1;
                }
                // Transient (rate-limit / network / 5xx / timeout) — leave unchecked, retry.
                Err(e) => tracing::warn!(target: "longbox_metron_link",
                    cv_id, error = %e, "metron series fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_metron_link", linked, checked_no_link, "metron link pass");
        // All transient this pass → back off before hot-retrying the same failing set.
        // Protects Metron's budget when the API is degraded.
        if linked + checked_no_link == 0 {
            tokio::time::sleep(IDLE_SLEEP).await;
        }
    }
}

/// Match our issues to Metron issue refs by issue number (deterministic,
/// zero-pad tolerant via `IssueNumber::matches`). Input rows are
/// `(issue_id, number, already_linked)`. Skips already-linked issues and
/// unmatched numbers; on duplicate Metron numbers, first wins. Returns
/// `(issue_id, metron_issue_id)` to persist.
fn match_issue_links(ours: &[(i64, String, bool)], metron: &[MetronIssueRef]) -> Vec<(i64, i64)> {
    ours.iter()
        .filter(|(_, _, already_linked)| !*already_linked)
        .filter_map(|(id, number, _)| {
            let n = IssueNumber::from(number.as_str());
            metron
                .iter()
                .find(|r| n.matches(&IssueNumber::from(r.issue_number.as_str())))
                .map(|r| (*id, r.metron_issue_id))
        })
        .collect()
}

/// Spawn the Metron issue-linking resolver (separate from the series-linker).
pub fn spawn_metron_issue_linker(db: Pool, metron: Arc<MetronClient>) {
    tokio::spawn(issue_link_loop(db, metron));
}

async fn issue_link_loop(db: Pool, metron: Arc<MetronClient>) {
    loop {
        let batch = match series_repo::list_metron_issue_link_candidates(&db, BATCH).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "longbox_metron_link", error = %e, "issue-link candidate query failed");
                tokio::time::sleep(IDLE_SLEEP).await;
                continue;
            }
        };
        if batch.is_empty() {
            tokio::time::sleep(IDLE_SLEEP).await;
            continue;
        }
        let (mut series_marked, mut issues_linked) = (0usize, 0usize);
        for (series_id, metron_id) in &batch {
            // A non-numeric metron_id is bad data — mark done so it can't churn.
            let Ok(mid) = metron_id.parse::<i64>() else {
                let _ = series_repo::mark_metron_issues_linked(&db, *series_id).await;
                series_marked += 1;
                continue;
            };
            match metron.fetch_issues_by_series_id(mid).await {
                Ok(refs) => match issue_repo::list_by_series(&db, *series_id).await {
                    Ok(ours) => {
                        let rows: Vec<(i64, String, bool)> = ours
                            .iter()
                            .map(|i| (i.id, i.number.clone(), i.metron_issue_id.is_some()))
                            .collect();
                        for (issue_id, m_issue_id) in match_issue_links(&rows, &refs) {
                            if series_repo_set_issue_ok(&db, issue_id, m_issue_id).await {
                                issues_linked += 1;
                            }
                        }
                        let _ = series_repo::mark_metron_issues_linked(&db, *series_id).await;
                        series_marked += 1;
                    }
                    Err(e) => tracing::warn!(target: "longbox_metron_link",
                            series_id, error = %e, "list_by_series failed; will retry"),
                },
                // Permanent per-series fetch error — mark done (no issues), no churn.
                Err(e) if is_terminal(&e) => {
                    let _ = series_repo::mark_metron_issues_linked(&db, *series_id).await;
                    series_marked += 1;
                    tracing::debug!(target: "longbox_metron_link",
                        series_id, error = %e, "terminal metron error; marked issues-linked (no data)");
                }
                // Transient — leave unmarked, retry next pass.
                Err(e) => tracing::warn!(target: "longbox_metron_link",
                    series_id, error = %e, "issue-list fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_metron_link", series_marked, issues_linked, "metron issue-link pass");
        // No progress this pass (all transient errors) — back off.
        if series_marked == 0 {
            tokio::time::sleep(IDLE_SLEEP).await;
        }
    }
}

/// Small helper so the loop body stays readable.
async fn series_repo_set_issue_ok(db: &Pool, issue_id: i64, metron_issue_id: i64) -> bool {
    issue_repo::set_metron_issue_id(db, issue_id, &metron_issue_id.to_string())
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use longbox_metron::{MetronError, MetronIssueRef};

    #[test]
    fn match_issue_links_by_number_deterministic() {
        let metron = vec![
            MetronIssueRef {
                metron_issue_id: 100,
                issue_number: "1".into(),
            },
            MetronIssueRef {
                metron_issue_id: 200,
                issue_number: "2".into(),
            },
            MetronIssueRef {
                metron_issue_id: 300,
                issue_number: "2".into(),
            }, // dup number
        ];
        // (issue_id, number, already_linked)
        let ours = vec![
            (11, "1".to_string(), false),   // -> 100
            (12, "002".to_string(), false), // zero-pad tolerant -> 200 (first dup wins)
            (13, "5".to_string(), false),   // no Metron match -> skipped
            (14, "1".to_string(), true),    // already linked -> skipped
        ];
        let links = match_issue_links(&ours, &metron);
        assert_eq!(links, vec![(11, 100), (12, 200)]);
    }

    #[test]
    fn is_terminal_classifies() {
        assert!(is_terminal(&MetronError::NotFound));
        assert!(is_terminal(&MetronError::Malformed {
            message: "bad payload".into(),
            raw_excerpt: None,
        }));
        assert!(is_terminal(&MetronError::Http {
            status: 404,
            body: String::new()
        }));
        assert!(is_terminal(&MetronError::Http {
            status: 400,
            body: String::new()
        }));
        // 429 is rate-limit — transient, not terminal.
        assert!(!is_terminal(&MetronError::Http {
            status: 429,
            body: String::new()
        }));
        // 5xx — transient.
        assert!(!is_terminal(&MetronError::Http {
            status: 500,
            body: String::new()
        }));
        assert!(!is_terminal(&MetronError::Timeout));
        assert!(!is_terminal(&MetronError::Auth));
        assert!(!is_terminal(&MetronError::RateLimited {
            retry_after_seconds: 5
        }));
    }
}
