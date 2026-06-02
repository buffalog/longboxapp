//! Phase B post-process pipeline: filesystem watcher + per-file
//! processing for new arrivals in the download folder.
//!
//! **Step 5 (this commit):** initial sweep + `notify` watcher feed
//! into an `mpsc` channel; a consumer task drains the channel and
//! logs each detected path. No file is opened, no DB write, no move.
//! The skip-pattern matcher (extension, in-progress suffixes,
//! dotfiles) lives in [`skip`].
//!
//! Step 6 will replace the consumer body with the real processing
//! pipeline (matcher → ComicInfo write → library convention → move →
//! catalog upsert). The detection plumbing here doesn't change.
//!
//! Architecture per `longbox-phase-b-prompt.md`:
//! - Hard crate boundary: `longbox-postprocess` owns its watcher and
//!   processing pipeline; `longbox-web` calls only [`start`].
//! - `longbox-comicvine` is not a dependency — Phase B never talks to
//!   CV. All metadata comes from the catalog.
//! - `longbox-scanner` is not a dependency — Phase B is event-driven,
//!   not walk-driven; the matcher lives in `longbox-core`.

pub mod config;
pub mod error;
pub mod intervention;
pub mod processor;
pub mod skip;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

pub use config::PostprocessConfig;
pub use error::{PostprocessError, Result};
pub use intervention::{InterventionReason, PendingIntervention, PendingInterventionsCache};

/// Channel capacity for the watcher → consumer queue. Bounded so a
/// runaway producer (e.g., SAB dumping thousands of files at once)
/// applies backpressure to the watcher rather than ballooning memory.
/// 4096 is generous — a SAB job completing 24 issues is well under
/// the cap, and the consumer drains continuously.
const CHANNEL_CAPACITY: usize = 4096;

/// Entry point called from `longbox-web` at startup when
/// `DOWNLOAD_WATCH_PATH` is configured and readable.
///
/// Setup-and-return shape: validates the watch path, spawns the
/// consumer + watcher tasks (each supervised so panics get logged
/// rather than silently dropping work), runs the initial sweep, then
/// returns. The detached background tasks live for the lifetime of
/// the process.
///
/// Step 5 body **detects only** — every path that survives the skip
/// filter gets one `phase_b.detected` log line and nothing else.
/// Step 6 replaces the consumer's per-path action with real
/// processing.
pub async fn start(
    config: PostprocessConfig,
    db: longbox_db::Pool,
    cache: Arc<PendingInterventionsCache>,
) -> Result<()> {
    // Fail fast on a bad config; web layer decides whether to
    // continue without Phase B (warn-and-skip semantics).
    std::fs::read_dir(&config.watch_path).map_err(|source| {
        PostprocessError::WatchPathUnreadable {
            path: config.watch_path.clone(),
            source,
        }
    })?;

    // Resolve library_root_id from the catalog. Spec'd signature is
    // `start(config, db)`, so this can't be a parameter — derive by
    // matching the configured library_root against existing rows.
    let library_root_id = resolve_library_root_id(&config.library_root, &db).await?;

    tracing::info!(
        target: "longbox_postprocess",
        watch_path = %config.watch_path.display(),
        library_root = %config.library_root.display(),
        library_root_id,
        "phase_b.started"
    );

    let (tx, rx) = mpsc::channel::<PathBuf>(CHANNEL_CAPACITY);

    // 1. Consumer first so it's draining by the time the sweep starts
    //    pushing — keeps the bounded channel from backpressuring the
    //    sweep on a large pre-existing folder.
    spawn_supervised(
        "consumer",
        consumer_task(
            rx,
            db,
            config.library_root.clone(),
            library_root_id,
            config.watch_path.clone(),
            Arc::clone(&cache),
        ),
    );

    // 2. Sweep pre-existing files. Spawned (not awaited) so start()
    //    returns promptly even on a large folder; the consumer drains
    //    in parallel.
    let sweep_tx = tx.clone();
    let sweep_root = config.watch_path.clone();
    spawn_supervised("sweep", initial_sweep(sweep_root, sweep_tx));

    // 3. Attach the watcher. Returns a Watcher handle that must
    //    outlive the watching period; park it inside a task that
    //    holds it for the program's lifetime. The cache clone lets
    //    the watcher callback evict entries on Remove / Rename-From
    //    events without going through the bounded channel.
    spawn_watcher(
        config.watch_path.clone(),
        config.poll_interval,
        tx,
        Arc::clone(&cache),
    )?;

    Ok(())
}

/// Match the configured library root against the catalog's existing
/// library_roots rows. Trailing-slash-tolerant: callers may have
/// normalized differently than the row stored. Phase A's bootstrap
/// inserts/finds the row by path; here we only look up.
async fn resolve_library_root_id(library_root: &Path, db: &longbox_db::Pool) -> Result<i64> {
    let rows = longbox_db::library_root_repo::list_all(db).await?;
    let configured = library_root.to_string_lossy();
    let configured_trimmed = configured.trim_end_matches('/');
    for row in rows {
        if row.path.trim_end_matches('/') == configured_trimmed {
            return Ok(row.id);
        }
    }
    Err(PostprocessError::LibraryRootNotInCatalog(
        library_root.to_path_buf(),
    ))
}

/// Walk the watch folder once at startup, enqueueing every CBZ that
/// passes the skip filter. Runs inside `spawn_blocking` because
/// walkdir is synchronous I/O.
async fn initial_sweep(root: PathBuf, tx: mpsc::Sender<PathBuf>) {
    let root_for_log = root.clone();
    let candidates = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut skipped = 0u32;
        for entry in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.into_path();
            match skip::should_skip(&path) {
                None => out.push(path),
                Some(reason) => {
                    skipped += 1;
                    tracing::debug!(
                        target: "longbox_postprocess",
                        path = %path.display(),
                        reason = ?reason,
                        "phase_b.skipped (initial sweep)"
                    );
                }
            }
        }
        (out, skipped)
    })
    .await;

    let (candidates, skipped) = match candidates {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                target: "longbox_postprocess",
                error = ?e,
                root = %root_for_log.display(),
                "phase_b.sweep_failed"
            );
            return;
        }
    };

    let candidate_count = candidates.len();
    for path in candidates {
        // Bounded channel: send awaits if full. Consumer is already
        // draining (spawned before sweep), so the only way this blocks
        // for long is if the consumer is stuck — in which case
        // applying backpressure here is exactly what we want.
        if tx.send(path).await.is_err() {
            tracing::warn!(
                target: "longbox_postprocess",
                "phase_b.consumer_closed (sweep aborted)"
            );
            return;
        }
    }

    tracing::info!(
        target: "longbox_postprocess",
        candidates = candidate_count,
        skipped,
        root = %root_for_log.display(),
        "phase_b.initial_sweep_complete"
    );
}

/// Per-event filter: turn a `notify::Event` into the set of paths
/// that should be considered for processing. Permissive on event
/// kind — the skip filter (extension, in-progress suffix, dotfile)
/// does the real gating. Step 6's idempotent upsert handles any
/// over-firing safely.
fn paths_from_event(event: &notify::Event) -> Vec<PathBuf> {
    use notify::EventKind;
    match event.kind {
        // File appeared (Linux inotify) or was renamed into the
        // watched dir (Modify::Name::To, RenameMode::Both on some
        // platforms gives the destination as paths[1]).
        EventKind::Create(_) | EventKind::Modify(_) => event.paths.clone(),
        _ => Vec::new(),
    }
}

/// Per-event filter for cache eviction: source paths that have
/// disappeared from the watch folder. Covers user manually deleting
/// the conflicting file (Remove) and user manually moving it elsewhere
/// (Modify::Name::From, sometimes Rename::Both with paths[0]). This is
/// *separate* from `paths_from_event` because the processing pipeline
/// must keep ignoring these kinds — Step 5's reasoning is unchanged.
/// Eviction is a cleanup signal that lives next to the watcher, not in
/// the processing channel.
fn eviction_paths_from_event(event: &notify::Event) -> Vec<PathBuf> {
    use notify::event::{ModifyKind, RenameMode};
    use notify::EventKind;
    match &event.kind {
        EventKind::Remove(_) => event.paths.clone(),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => event.paths.clone(),
        // Rename::Both reports source as paths[0] and dest as paths[1].
        // The source needs eviction (user moved the conflicting file
        // away); the dest will hit the normal processing channel via
        // paths_from_event.
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            event.paths.first().cloned().into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// Stand up the `notify` watcher. The returned `Watcher` is parked
/// inside a long-lived task because dropping it stops the watch.
///
/// **Why `PollWatcher` and not `recommended_watcher`:** LongBox runs
/// inside a Docker Desktop container on macOS, mounting `/watch` from
/// the host via virtiofs. inotify is blind to host writes through
/// virtiofs — a CBZ landing in `/watch` from outside the container
/// never fires a Create event, so a file downloaded by the host took
/// 44+ minutes to be noticed before the swap. `PollWatcher` walks the
/// directory on a fixed interval and diffs mtime+size; works on every
/// platform LongBox might ever run on, at the cost of up-to-interval
/// detection latency. `PollWatcher` implements the same `Watcher`
/// trait and emits the same `EventKind::Create / Modify / Remove`
/// events, so the closure body below is untouched.
///
/// **Revert path:** on a native-Linux deploy where inotify works,
/// swap `PollWatcher::new(handler, cfg)` back to
/// `notify::recommended_watcher(handler)` — one line. The poll
/// interval setting is then dead but harmless.
///
/// `PollWatcher`'s first poll silently populates its internal
/// `PathData` map; pre-existing files do NOT fire as Create events
/// (the `is_initial` flag only emits to the optional
/// `ScanEventHandler`). The `initial_sweep` task above remains
/// responsible for catching files that exist at boot.
fn spawn_watcher(
    watch_path: PathBuf,
    poll_interval: Duration,
    tx: mpsc::Sender<PathBuf>,
    cache: Arc<PendingInterventionsCache>,
) -> Result<()> {
    // notify's callback runs on its own thread; we hand events to the
    // tokio channel via `try_send`. `Arc<Sender>` lets the closure
    // own a clone without lifetime gymnastics.
    let tx = Arc::new(tx);
    let tx_for_cb = Arc::clone(&tx);

    let poll_config = notify::Config::default().with_poll_interval(poll_interval);
    let mut watcher = notify::PollWatcher::new(
        move |res: notify::Result<notify::Event>| {
        let event = match res {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    target: "longbox_postprocess",
                    error = %e,
                    "phase_b.watcher_event_error"
                );
                return;
            }
        };

        // Eviction first: a Remove/Rename-From for a path that's in the
        // pending-intervention cache must clear that entry, even if
        // skip::should_skip would filter the path out of processing.
        // The cache is a per-source-path map; eviction is keyed on the
        // path string, not on whether it's currently CBZ-shaped.
        for path in eviction_paths_from_event(&event) {
            cache.remove_by_source_path(&path);
        }

        for path in paths_from_event(&event) {
            if let Some(reason) = skip::should_skip(&path) {
                tracing::debug!(
                    target: "longbox_postprocess",
                    path = %path.display(),
                    reason = ?reason,
                    "phase_b.skipped (watcher)"
                );
                continue;
            }
            match tx_for_cb.try_send(path.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        target: "longbox_postprocess",
                        path = %path.display(),
                        "phase_b.channel_full (event dropped — consumer is slow)"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    // Consumer task has exited; watcher is irrelevant.
                    // No point logging on every subsequent event; it
                    // would spam. One log here and the watcher will
                    // keep firing harmlessly until the process exits.
                    tracing::warn!(
                        target: "longbox_postprocess",
                        "phase_b.consumer_closed (event dropped)"
                    );
                }
            }
        }
        },
        poll_config,
    )?;

    watcher.watch(&watch_path, RecursiveMode::Recursive)?;

    // Park the watcher inside a long-lived task. The task's body holds
    // the Watcher value; the task itself blocks forever on a never-
    // resolving future. When the process exits, the task is dropped
    // and the watcher with it.
    tokio::spawn(async move {
        let _watcher = watcher;
        std::future::pending::<()>().await;
    });

    Ok(())
}

/// Consume detected paths and run the processing pipeline on each.
/// Per-file errors are logged at WARN and the loop continues — one bad
/// file must not stall the queue. Returning errors from this task body
/// would have the supervisor log a panic; per-file failures are not
/// panic-worthy.
///
/// Cache discipline: a successful `Imported` or `Unsorted` evicts any
/// stale entry for the same source path (self-healing — user resolved
/// a prior conflict and the file re-fired through notify). `Conflict`
/// and `Failed` push into the cache so the dashboard counter reflects
/// stuck files. Bubbled-up `Err` (DB failures, source read errors
/// before classification) are logged but not cached — the brief
/// scopes the dashboard to Conflict / ComicInfo-write / Move failures.
async fn consumer_task(
    mut rx: mpsc::Receiver<PathBuf>,
    db: longbox_db::Pool,
    library_root: PathBuf,
    library_root_id: i64,
    watch_root: PathBuf,
    cache: Arc<PendingInterventionsCache>,
) {
    // SABnzbd deposits downloads as `{watch_root}/{job_name}/{file.cbr}`.
    // After processing moves the file out, the now-empty `{job_name}/`
    // is left dangling. Canonicalize the watch root once here so the
    // per-file equality check that protects the root itself is stable
    // against symlinks / trailing-slash drift in the configured path.
    let canonical_watch_root = std::fs::canonicalize(&watch_root).unwrap_or(watch_root);

    while let Some(path) = rx.recv().await {
        // Skip-filter is run at the producer side (sweep + watcher),
        // but a path that's been removed between detection and now
        // would show up here. Re-checking the filter is cheap.
        if let Some(reason) = skip::should_skip(&path) {
            tracing::debug!(
                target: "longbox_postprocess",
                path = %path.display(),
                reason = ?reason,
                "phase_b.skipped (post-detection re-check)"
            );
            continue;
        }
        match processor::process_one(&path, &library_root, library_root_id, &db).await {
            Ok(outcome) => {
                if matches!(
                    &outcome,
                    processor::Outcome::Imported { .. } | processor::Outcome::Unsorted { .. }
                ) {
                    cleanup_empty_parent(&path, &canonical_watch_root);
                }
                apply_outcome_to_cache(&cache, &path, outcome);
            }
            Err(e) => {
                tracing::warn!(
                    target: "longbox_postprocess",
                    path = %path.display(),
                    err = %e,
                    "phase_b.failed"
                );
            }
        }
    }
    tracing::info!(
        target: "longbox_postprocess",
        "phase_b.consumer_exited (channel closed)"
    );
}

/// If a processed file lived in a subdirectory of the watch root
/// (the SABnzbd `{watch_root}/{job_name}/` shape), remove the parent
/// when it's now empty. Best-effort: filesystem races (a sibling file
/// reappearing, permission denied, parent already gone) log at debug
/// and skip rather than failing the import that already succeeded.
///
/// The canonicalized-equality guard against `watch_root` itself is
/// load-bearing: deleting the watch folder would silently break the
/// pipeline. Files dropped directly into the watch root therefore
/// short-circuit before any `read_dir` work.
fn cleanup_empty_parent(source: &Path, canonical_watch_root: &Path) {
    let Some(parent) = source.parent() else {
        return;
    };

    let canonical_parent = match std::fs::canonicalize(parent) {
        Ok(p) => p,
        Err(_) => return,
    };

    if canonical_parent == canonical_watch_root {
        return;
    }

    let entries = match std::fs::read_dir(&canonical_parent) {
        Ok(e) => e,
        Err(_) => return,
    };
    if entries.count() > 0 {
        return;
    }

    match std::fs::remove_dir(&canonical_parent) {
        Ok(()) => {
            tracing::info!(
                target: "longbox_postprocess",
                dir = %canonical_parent.display(),
                "phase_b.watch_dir_cleaned"
            );
        }
        Err(e) => {
            tracing::debug!(
                target: "longbox_postprocess",
                dir = %canonical_parent.display(),
                err = %e,
                "phase_b.watch_dir_cleanup_failed"
            );
        }
    }
}

/// Translate an `Outcome` into the right cache mutation. Pulled out
/// of `consumer_task` so tests can drive cache transitions directly
/// without standing up the channel + processor.
fn apply_outcome_to_cache(
    cache: &PendingInterventionsCache,
    source: &std::path::Path,
    outcome: processor::Outcome,
) {
    use processor::Outcome;
    match outcome {
        Outcome::Imported { .. } | Outcome::Unsorted { .. } => {
            cache.remove_by_source_path(source);
        }
        Outcome::Conflict { target, size } => {
            cache.push(PendingIntervention {
                source_path: source.to_path_buf(),
                target_path: target,
                reason: InterventionReason::Conflict,
                size,
                last_attempt: time::OffsetDateTime::now_utc(),
            });
        }
        Outcome::Failed {
            reason,
            target,
            size,
        } => {
            cache.push(PendingIntervention {
                source_path: source.to_path_buf(),
                target_path: target,
                reason,
                size,
                last_attempt: time::OffsetDateTime::now_utc(),
            });
        }
    }
}

/// Spawn a future as a tokio task and log on completion. Detached
/// tasks normally swallow panics — the inner spawn's `JoinHandle`
/// surfaces them to the outer spawn, which logs via tracing.
///
/// `name` becomes the `task` field in log events so a panic shows up
/// as `task=consumer` / `task=sweep` / etc.
fn spawn_supervised<F>(name: &'static str, fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = tokio::spawn(fut);
    tokio::spawn(async move {
        match handle.await {
            Ok(()) => tracing::debug!(
                target: "longbox_postprocess",
                task = name,
                "phase_b.task_exited"
            ),
            Err(e) if e.is_panic() => tracing::error!(
                target: "longbox_postprocess",
                task = name,
                error = ?e,
                "phase_b.task_panicked"
            ),
            Err(e) => tracing::warn!(
                target: "longbox_postprocess",
                task = name,
                error = ?e,
                "phase_b.task_cancelled"
            ),
        }
    });
}

/// Public for the doctest harness. Used only by the type checker.
#[doc(hidden)]
pub fn _types_compile(_: PostprocessConfig, _: PendingIntervention, _: InterventionReason) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn start_errors_on_missing_watch_path() {
        let pool = longbox_db::open(":memory:").await.unwrap();
        let config = PostprocessConfig {
            watch_path: PathBuf::from("/nonexistent-longbox-test-path"),
            library_root: PathBuf::from("/tmp/longbox-test-library"),
            poll_interval: Duration::from_secs(30),
        };
        let err = start(config, pool, Arc::new(PendingInterventionsCache::new()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PostprocessError::WatchPathUnreadable { .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn start_succeeds_on_readable_empty_dir() {
        let pool = longbox_db::open(":memory:").await.unwrap();
        let tmp = TempDir::new().unwrap();
        // Phase B's start() resolves library_root_id by matching the
        // configured library_root path against catalog rows; seed one.
        longbox_db::library_root_repo::insert(
            &pool,
            longbox_db::NewLibraryRoot {
                path: tmp.path().to_string_lossy().into_owned(),
            },
        )
        .await
        .unwrap();
        let config = PostprocessConfig {
            watch_path: tmp.path().to_path_buf(),
            library_root: tmp.path().to_path_buf(),
            poll_interval: Duration::from_secs(30),
        };
        start(config, pool, Arc::new(PendingInterventionsCache::new()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn start_errors_when_library_root_not_in_catalog() {
        let pool = longbox_db::open(":memory:").await.unwrap();
        let tmp = TempDir::new().unwrap();
        let config = PostprocessConfig {
            watch_path: tmp.path().to_path_buf(),
            library_root: tmp.path().to_path_buf(),
            poll_interval: Duration::from_secs(30),
        };
        let err = start(config, pool, Arc::new(PendingInterventionsCache::new()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PostprocessError::LibraryRootNotInCatalog(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn apply_outcome_caches_conflict() {
        use processor::Outcome;
        let cache = PendingInterventionsCache::new();
        let source = PathBuf::from("/watch/saga 001.cbz");
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Conflict {
                target: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
                size: 4096,
            },
        );
        let snap = cache.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].source_path, source);
        assert_eq!(snap[0].reason, InterventionReason::Conflict);
        assert_eq!(snap[0].size, 4096);
    }

    #[test]
    fn apply_outcome_caches_failed_with_stage_reason() {
        use processor::Outcome;
        let cache = PendingInterventionsCache::new();
        let source = PathBuf::from("/watch/saga 001.cbz");
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Failed {
                reason: InterventionReason::MoveFailed("EXDEV".into()),
                target: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
                size: 4096,
            },
        );
        let snap = cache.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(matches!(
            snap[0].reason,
            InterventionReason::MoveFailed(ref m) if m == "EXDEV"
        ));
    }

    #[test]
    fn apply_outcome_evicts_on_imported() {
        use processor::Outcome;
        let cache = PendingInterventionsCache::new();
        let source = PathBuf::from("/watch/saga 001.cbz");
        // Pre-seed a stuck entry, then run a successful import for the
        // same source path — self-healing behavior.
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Conflict {
                target: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
                size: 4096,
            },
        );
        assert_eq!(cache.len(), 1);
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Imported {
                target: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
                series_id: 1,
                issue_id: 2,
                file_id: 3,
            },
        );
        assert!(cache.is_empty(), "Imported should evict the stale entry");
    }

    #[test]
    fn apply_outcome_evicts_on_unsorted() {
        use processor::Outcome;
        let cache = PendingInterventionsCache::new();
        let source = PathBuf::from("/watch/mystery.cbz");
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Failed {
                reason: InterventionReason::ComicInfoWriteFailed("permission".into()),
                target: PathBuf::from("/lib/Unknown.cbz"),
                size: 1,
            },
        );
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Unsorted {
                target: PathBuf::from("/lib/_unsorted/mystery.cbz"),
                file_id: 7,
            },
        );
        assert!(cache.is_empty(), "Unsorted should evict the stale entry");
    }

    #[test]
    fn eviction_paths_from_event_catches_remove_and_rename_from() {
        use notify::event::{Event, EventKind, ModifyKind, RemoveKind, RenameMode};
        let with_paths = |kind: EventKind, paths: Vec<PathBuf>| Event {
            kind,
            paths,
            attrs: Default::default(),
        };

        assert_eq!(
            eviction_paths_from_event(&with_paths(
                EventKind::Remove(RemoveKind::File),
                vec![PathBuf::from("/x.cbz")],
            ))
            .len(),
            1
        );
        assert_eq!(
            eviction_paths_from_event(&with_paths(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                vec![PathBuf::from("/from.cbz")],
            ))
            .len(),
            1
        );
        // Rename::Both: source path is paths[0]; we evict that.
        assert_eq!(
            eviction_paths_from_event(&with_paths(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                vec![PathBuf::from("/from.cbz"), PathBuf::from("/to.cbz")],
            )),
            vec![PathBuf::from("/from.cbz")]
        );
        // Create events do not evict.
        assert!(eviction_paths_from_event(&with_paths(
            EventKind::Create(notify::event::CreateKind::File),
            vec![PathBuf::from("/x.cbz")],
        ))
        .is_empty());
    }

    #[test]
    fn cleanup_empty_parent_removes_empty_job_subdir() {
        // SAB shape: {watch}/{job}/{file}. After the file is moved out
        // by the processor, the source path's parent is the empty job
        // dir — that is exactly what we want to clean up.
        let watch = TempDir::new().unwrap();
        let watch_root = std::fs::canonicalize(watch.path()).unwrap();
        let job_dir = watch.path().join("saga-job-42");
        std::fs::create_dir(&job_dir).unwrap();
        // No file created — simulates the post-move state.
        let phantom_source = job_dir.join("saga 001.cbz");

        cleanup_empty_parent(&phantom_source, &watch_root);

        assert!(!job_dir.exists(), "empty job dir should be deleted");
    }

    #[test]
    fn cleanup_empty_parent_refuses_to_delete_the_watch_root() {
        // A file dropped directly into the watch root has the watch
        // root itself as its parent. Deleting it would break Phase B
        // until the user recreated the folder — the load-bearing
        // guard at the top of `cleanup_empty_parent`.
        let watch = TempDir::new().unwrap();
        let watch_root = std::fs::canonicalize(watch.path()).unwrap();
        let phantom_source = watch.path().join("loose-file.cbz");

        cleanup_empty_parent(&phantom_source, &watch_root);

        assert!(watch.path().exists(), "watch root must not be deleted");
    }

    #[test]
    fn cleanup_empty_parent_skips_nonempty_dirs() {
        // If a sibling file is still in the job dir (multi-file job
        // where one was unmatched and stuck for intervention), the
        // cleanup must NOT touch the dir.
        let watch = TempDir::new().unwrap();
        let watch_root = std::fs::canonicalize(watch.path()).unwrap();
        let job_dir = watch.path().join("two-file-job");
        std::fs::create_dir(&job_dir).unwrap();
        let sibling = job_dir.join("still-here.cbz");
        std::fs::write(&sibling, b"stub").unwrap();
        let phantom_source = job_dir.join("moved.cbz");

        cleanup_empty_parent(&phantom_source, &watch_root);

        assert!(job_dir.exists(), "non-empty job dir must survive");
        assert!(sibling.exists(), "sibling file must survive");
    }

    #[test]
    fn cleanup_empty_parent_tolerates_missing_parent() {
        // If the parent vanished between the move and the cleanup
        // (another process, user intervention), the function must
        // silently no-op rather than panic.
        let watch = TempDir::new().unwrap();
        let watch_root = std::fs::canonicalize(watch.path()).unwrap();
        let phantom_source = watch.path().join("gone-job/gone.cbz");

        cleanup_empty_parent(&phantom_source, &watch_root);
        // Reaching this line without panic is the assertion.
    }

    #[test]
    fn paths_from_event_filters_to_relevant_kinds() {
        use notify::event::{Event, EventKind, ModifyKind, RenameMode};
        let with_paths = |kind: EventKind| Event {
            kind,
            paths: vec![PathBuf::from("/x/y.cbz")],
            attrs: Default::default(),
        };

        assert_eq!(
            paths_from_event(&with_paths(EventKind::Create(
                notify::event::CreateKind::File
            )))
            .len(),
            1
        );
        assert_eq!(
            paths_from_event(&with_paths(EventKind::Modify(ModifyKind::Name(
                RenameMode::To
            ))))
            .len(),
            1
        );
        assert_eq!(
            paths_from_event(&with_paths(EventKind::Remove(
                notify::event::RemoveKind::File
            )))
            .len(),
            0
        );
    }
}
