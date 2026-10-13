//! Catalog refresh: getting issues ComicVine has published INTO the
//! catalog, so the pull engine (which never talks to CV) can see them.
//!
//! Two modes, both upserting by `cv_issue_id` through [`apply_issues`]:
//!
//! - **Delta**, inline at the top of every pull sweep via the scheduler's
//!   `PreSweep` hook: one `store_date`-window calendar query, kept to
//!   volumes the catalog tracks (all series, not just the pull list, so
//!   Missing and solicited stay accurate). About ten CV pages.
//! - **Full**, its own background task over every pull-list series, one
//!   `fetch_issues` each. Runs at boot when it has never run, then weekly,
//!   and on `POST /api/refresh/all`. It is not inside the sweep: a series
//!   that gains issues is rematched and searched as soon as it is done, so
//!   downloads flow during the ~2h pass instead of after it.
//!
//! All CV calls go through the process-wide [`BackgroundCvClient`], so a
//! refresh queues behind (and alongside) enrichment and the credits
//! resolver instead of starving interactive calls.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use longbox_cv_enrichment::BackgroundCvClient;
use longbox_db::{issue_repo, pull_list_repo, series_repo, settings_repo, DbError, NewIssue, Pool};
use sqlx::SqliteConnection;
use time::{Duration, OffsetDateTime};

/// How often the full pass reruns.
const FULL_REFRESH_EVERY: Duration = Duration::days(7);

/// What one [`apply_issues`] call did.
#[derive(Debug, Default)]
pub struct ApplyCounts {
    pub inserted: usize,
    pub updated: usize,
    /// Rows whose `store_date` went from NULL to a value.
    pub store_dates_set: usize,
    /// Inserts refused because the series already holds that issue
    /// number on a row with no `cv_issue_id` (a number-only row from a
    /// bulk-convert link). Logged and skipped; nothing is overwritten.
    pub number_collisions: usize,
    /// Series that gained at least one row.
    pub series_with_inserts: HashSet<i64>,
}

/// Upsert CV issues by `cv_issue_id` inside the caller's transaction.
/// `upsert_by_cv_id` is null-safe, so a sparse source can't blank data.
///
/// A number collision (see [`ApplyCounts::number_collisions`]) is skipped
/// per issue. SQLite rolls back only the failed statement, so the rest of
/// the batch and the transaction stand.
pub async fn apply_issues(
    conn: &mut SqliteConnection,
    issues: Vec<NewIssue>,
) -> Result<ApplyCounts, DbError> {
    let mut counts = ApplyCounts::default();
    for input in issues {
        let Some(cv_issue_id) = input.cv_issue_id else {
            continue;
        };
        let prior = issue_repo::find_by_cv_issue_id(&mut *conn, cv_issue_id).await?;
        let (series_id, number) = (input.series_id, input.number.clone());
        match issue_repo::upsert_by_cv_id(&mut *conn, input).await {
            Ok(row) => {
                match &prior {
                    None => {
                        counts.inserted += 1;
                        counts.series_with_inserts.insert(row.series_id);
                    }
                    Some(_) => counts.updated += 1,
                }
                let had = prior.as_ref().is_some_and(|p| p.store_date.is_some());
                if !had && row.store_date.is_some() {
                    counts.store_dates_set += 1;
                }
            }
            Err(DbError::UniqueViolation { .. }) => {
                counts.number_collisions += 1;
                tracing::warn!(
                    target: "longbox_web",
                    series_id,
                    cv_issue_id,
                    number = %number,
                    "refresh.number_collision (series already holds this number without a cv_issue_id; skipped)"
                );
            }
            Err(e) => return Err(e),
        }
    }
    Ok(counts)
}

/// One pass's tallies, logged as `refresh.pass_complete`.
#[derive(Debug, Default)]
pub struct PassSummary {
    pub series_refreshed: usize,
    pub issues_inserted: usize,
    pub issues_updated: usize,
    pub store_dates_set: usize,
    pub number_collisions: usize,
    pub cv_errors: usize,
    pub db_errors: usize,
}

impl PassSummary {
    fn add(&mut self, c: &ApplyCounts) {
        self.issues_inserted += c.inserted;
        self.issues_updated += c.updated;
        self.store_dates_set += c.store_dates_set;
        self.number_collisions += c.number_collisions;
    }

    fn log(&self, mode: &'static str) {
        tracing::info!(
            target: "longbox_web",
            mode,
            series_refreshed = self.series_refreshed,
            issues_inserted = self.issues_inserted,
            issues_updated = self.issues_updated,
            store_dates_set = self.store_dates_set,
            number_collisions = self.number_collisions,
            cv_errors = self.cv_errors,
            db_errors = self.db_errors,
            "refresh.pass_complete"
        );
    }
}

/// Everything a refresh needs. Cheap to clone.
#[derive(Clone)]
pub struct RefreshDeps {
    pub db: Pool,
    pub bg: BackgroundCvClient,
    pub scanner: Arc<longbox_scanner::Scanner>,
    pub pull_search: longbox_pull::PullSearchHandle,
}

/// Handle to the full-refresh task, parked in `AppState`. Cheap to clone.
#[derive(Clone)]
pub struct RefreshHandle {
    inner: Arc<Inner>,
}

struct Inner {
    deps: RefreshDeps,
    running: AtomicBool,
    done: AtomicUsize,
    total: AtomicUsize,
}

impl RefreshHandle {
    pub fn new(deps: RefreshDeps) -> Self {
        Self {
            inner: Arc::new(Inner {
                deps,
                running: AtomicBool::new(false),
                done: AtomicUsize::new(0),
                total: AtomicUsize::new(0),
            }),
        }
    }

    /// `(done, total)` series while a full refresh runs, else `None`.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.inner.running.load(Ordering::SeqCst).then(|| {
            (
                self.inner.done.load(Ordering::SeqCst),
                self.inner.total.load(Ordering::SeqCst),
            )
        })
    }

    /// Start a full refresh. `false` when one is already running.
    pub fn try_start_full(&self) -> bool {
        if self
            .inner
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        self.inner.done.store(0, Ordering::SeqCst);
        self.inner.total.store(0, Ordering::SeqCst);
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            // Clears `running` even if the pass panics, so a crashed pass
            // can't wedge Refresh-all behind a permanent 409.
            struct Reset<'a>(&'a AtomicBool);
            impl Drop for Reset<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _reset = Reset(&inner.running);
            run_full(&inner).await;
        });
        true
    }

    /// Start a full refresh when it has never completed or the last one
    /// is a week old. Called at boot and before every sweep.
    pub async fn start_full_if_due(&self) {
        match last_full_refresh_at(&self.inner.deps.db).await {
            Ok(last) => {
                let due = last.map_or(true, |t| {
                    OffsetDateTime::now_utc() - t >= FULL_REFRESH_EVERY
                });
                if due {
                    self.try_start_full();
                }
            }
            Err(e) => tracing::warn!(
                target: "longbox_web",
                error = %e,
                "refresh.due_check_failed"
            ),
        }
    }

    /// The `PreSweep` body: run the delta inline, then kick off the full
    /// pass if it is due. The sweep's Phase 2 follows.
    pub async fn before_sweep(&self) {
        run_delta(&self.inner.deps).await.log("delta");
        self.start_full_if_due().await;
    }
}

/// When the last full refresh completed. `None` when it never has, or
/// the stored value is unreadable (refreshing again is the safe side).
pub async fn last_full_refresh_at(db: &Pool) -> Result<Option<OffsetDateTime>, DbError> {
    let raw = settings_repo::get(db, settings_repo::KEY_LAST_FULL_REFRESH_AT).await?;
    Ok(raw
        .and_then(|r| r.parse::<i64>().ok())
        .and_then(|secs| OffsetDateTime::from_unix_timestamp(secs).ok()))
}

/// After a series gains issues: rematch first, so needs-review files can
/// claim the new rows before a search would download duplicates; then
/// search it now if it's an active pull-list series. Awaited, not
/// spawned, for the same ordering reason.
async fn after_insert(deps: &RefreshDeps, series_id: i64, search: bool) {
    match deps.scanner.rematch_for_series(series_id).await {
        Ok(report) => tracing::info!(
            target: "longbox_web",
            series_id,
            matched = report.matched_owned,
            caller = "catalog_refresh",
            "auto-rematch completed"
        ),
        // Another scan holds the scanner; the scan itself will match.
        Err(e) => tracing::warn!(
            target: "longbox_web",
            series_id,
            error = %e,
            caller = "catalog_refresh",
            "auto-rematch skipped"
        ),
    }
    if !search {
        return;
    }
    match pull_list_repo::get(&deps.db, series_id).await {
        // `sweep_single_series` doesn't check `paused`; the daily sweep
        // filters it via `list_active`, so mirror that here.
        Ok(Some(entry)) if !entry.paused => {
            deps.pull_search.try_start(series_id).await;
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(
            target: "longbox_web",
            series_id,
            error = %e,
            "refresh.pull_list_lookup_failed"
        ),
    }
}

/// Delta: upsert every CV issue with a `store_date` in the configured
/// window that belongs to a tracked volume.
pub async fn run_delta(deps: &RefreshDeps) -> PassSummary {
    let mut summary = PassSummary::default();
    let back: i64 =
        settings_repo::get_or_default(&deps.db, settings_repo::KEY_REFRESH_DELTA_DAYS_BACK, 14)
            .await
            .unwrap_or(14);
    let ahead: i64 =
        settings_repo::get_or_default(&deps.db, settings_repo::KEY_REFRESH_DELTA_DAYS_AHEAD, 28)
            .await
            .unwrap_or(28);
    let today = OffsetDateTime::now_utc().date();
    let from = (today - Duration::days(back)).to_string();
    let to = (today + Duration::days(ahead)).to_string();

    let items = match deps.bg.fetch_release_calendar(&from, &to).await {
        Ok(items) => items,
        Err(e) => {
            summary.cv_errors += 1;
            tracing::warn!(target: "longbox_web", error = %e, from, to, "refresh.delta_fetch_failed");
            return summary;
        }
    };
    let tracked: HashMap<i64, i64> = match series_repo::find_all(&deps.db).await {
        Ok(all) => all
            .into_iter()
            .filter_map(|s| s.cv_id.map(|cv| (cv, s.id)))
            .collect(),
        Err(e) => {
            summary.db_errors += 1;
            tracing::warn!(target: "longbox_web", error = %e, "refresh.delta_series_load_failed");
            return summary;
        }
    };
    let issues: Vec<NewIssue> = items
        .into_iter()
        .filter_map(|item| {
            let series_id = *tracked.get(&item.cv_volume_id)?;
            Some(NewIssue {
                series_id,
                cv_issue_id: Some(item.cv_issue_id),
                metron_issue_id: None,
                number: item.issue_number,
                title: item.name,
                cover_date: item.cover_date,
                summary: item.description,
                cover_url: item.cover_url,
                store_date: Some(item.store_date),
            })
        })
        .collect();
    summary.series_refreshed = issues
        .iter()
        .map(|i| i.series_id)
        .collect::<HashSet<_>>()
        .len();

    let counts = match apply_in_tx(&deps.db, issues).await {
        Ok(c) => c,
        Err(e) => {
            summary.db_errors += 1;
            tracing::warn!(target: "longbox_web", error = %e, "refresh.delta_apply_failed");
            return summary;
        }
    };
    summary.add(&counts);
    // No search here: the sweep's Phase 2 runs right after the delta.
    for series_id in &counts.series_with_inserts {
        after_insert(deps, *series_id, false).await;
    }
    summary
}

async fn apply_in_tx(db: &Pool, issues: Vec<NewIssue>) -> Result<ApplyCounts, DbError> {
    let mut tx = db.begin().await?;
    let counts = apply_issues(&mut tx, issues).await?;
    tx.commit().await?;
    Ok(counts)
}

/// Full: refresh every pull-list series' issue list from CV, one series
/// at a time. One bad volume logs and the pass continues.
async fn run_full(inner: &Inner) {
    let deps = &inner.deps;
    let mut summary = PassSummary::default();
    let targets: Vec<(i64, i64)> = match pull_list_targets(&deps.db).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(target: "longbox_web", error = %e, "refresh.full_target_load_failed");
            return;
        }
    };
    inner.total.store(targets.len(), Ordering::SeqCst);
    tracing::info!(target: "longbox_web", series = targets.len(), "refresh.full_started");

    for (series_id, cv_id) in targets {
        match deps.bg.fetch_issues(cv_id).await {
            Ok(cv_issues) => {
                let issues = cv_issues
                    .into_iter()
                    .map(|i| NewIssue {
                        series_id,
                        cv_issue_id: Some(i.cv_issue_id),
                        metron_issue_id: None,
                        number: i.issue_number,
                        title: i.name,
                        cover_date: i.cover_date,
                        summary: i.description,
                        cover_url: i.cover_url,
                        store_date: i.store_date,
                    })
                    .collect();
                match apply_in_tx(&deps.db, issues).await {
                    Ok(counts) => {
                        summary.series_refreshed += 1;
                        summary.add(&counts);
                        if counts.inserted > 0 {
                            after_insert(deps, series_id, true).await;
                        }
                    }
                    Err(e) => {
                        summary.db_errors += 1;
                        tracing::warn!(target: "longbox_web", series_id, cv_id, error = %e, "refresh.full_apply_failed");
                    }
                }
            }
            Err(e) => {
                summary.cv_errors += 1;
                tracing::warn!(target: "longbox_web", series_id, cv_id, error = %e, "refresh.full_fetch_failed");
            }
        }
        inner.done.fetch_add(1, Ordering::SeqCst);
    }

    // Written on completion even with per-series failures (counted
    // above); otherwise one dead volume would rerun the pass every sweep.
    let now = OffsetDateTime::now_utc().unix_timestamp().to_string();
    if let Err(e) =
        settings_repo::set(&deps.db, settings_repo::KEY_LAST_FULL_REFRESH_AT, &now).await
    {
        tracing::warn!(target: "longbox_web", error = %e, "refresh.last_full_write_failed");
    }
    summary.log("full");
}

/// `(series_id, cv_id)` for every pull-list series with a CV id. Paused
/// series are refreshed too (metadata is harmless); only the search
/// after an insert skips them.
async fn pull_list_targets(db: &Pool) -> Result<Vec<(i64, i64)>, DbError> {
    let cv_ids: HashMap<i64, i64> = series_repo::find_all(db)
        .await?
        .into_iter()
        .filter_map(|s| s.cv_id.map(|cv| (s.id, cv)))
        .collect();
    Ok(pull_list_repo::list_all(db)
        .await?
        .into_iter()
        .filter_map(|e| cv_ids.get(&e.series_id).map(|cv| (e.series_id, *cv)))
        .collect())
}
