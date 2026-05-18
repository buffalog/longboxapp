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

use std::path::PathBuf;
use std::sync::Arc;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

pub use config::PostprocessConfig;
pub use error::{PostprocessError, Result};
pub use intervention::{InterventionReason, PendingIntervention};

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
pub async fn start(config: PostprocessConfig, db: longbox_db::Pool) -> Result<()> {
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
        consumer_task(rx, db, config.library_root.clone(), library_root_id),
    );

    // 2. Sweep pre-existing files. Spawned (not awaited) so start()
    //    returns promptly even on a large folder; the consumer drains
    //    in parallel.
    let sweep_tx = tx.clone();
    let sweep_root = config.watch_path.clone();
    spawn_supervised("sweep", initial_sweep(sweep_root, sweep_tx));

    // 3. Attach the watcher. Returns a Watcher handle that must
    //    outlive the watching period; park it inside a task that
    //    holds it for the program's lifetime.
    spawn_watcher(config.watch_path.clone(), tx)?;

    Ok(())
}

/// Match the configured library root against the catalog's existing
/// library_roots rows. Trailing-slash-tolerant: callers may have
/// normalized differently than the row stored. Phase A's bootstrap
/// inserts/finds the row by path; here we only look up.
async fn resolve_library_root_id(library_root: &PathBuf, db: &longbox_db::Pool) -> Result<i64> {
    let rows = longbox_db::library_root_repo::list_all(db).await?;
    let configured = library_root.to_string_lossy();
    let configured_trimmed = configured.trim_end_matches('/');
    for row in rows {
        if row.path.trim_end_matches('/') == configured_trimmed {
            return Ok(row.id);
        }
    }
    Err(PostprocessError::LibraryRootNotInCatalog(library_root.clone()))
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

/// Stand up the `notify` watcher. The returned `Watcher` is parked
/// inside a long-lived task because dropping it stops the watch.
fn spawn_watcher(watch_path: PathBuf, tx: mpsc::Sender<PathBuf>) -> Result<()> {
    // notify's callback runs on its own thread; we hand events to the
    // tokio channel via `try_send`. `Arc<Sender>` lets the closure
    // own a clone without lifetime gymnastics.
    let tx = Arc::new(tx);
    let tx_for_cb = Arc::clone(&tx);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
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
    })?;

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
async fn consumer_task(
    mut rx: mpsc::Receiver<PathBuf>,
    db: longbox_db::Pool,
    library_root: PathBuf,
    library_root_id: i64,
) {
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
            Ok(_outcome) => {} // process_one logs its own outcome
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
        };
        let err = start(config, pool).await.unwrap_err();
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
        };
        start(config, pool).await.unwrap();
    }

    #[tokio::test]
    async fn start_errors_when_library_root_not_in_catalog() {
        let pool = longbox_db::open(":memory:").await.unwrap();
        let tmp = TempDir::new().unwrap();
        let config = PostprocessConfig {
            watch_path: tmp.path().to_path_buf(),
            library_root: tmp.path().to_path_buf(),
        };
        let err = start(config, pool).await.unwrap_err();
        assert!(
            matches!(err, PostprocessError::LibraryRootNotInCatalog(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn paths_from_event_filters_to_relevant_kinds() {
        use notify::event::{Event, EventKind, ModifyKind, RenameMode};
        let with_paths =
            |kind: EventKind| Event { kind, paths: vec![PathBuf::from("/x/y.cbz")], attrs: Default::default() };

        assert_eq!(
            paths_from_event(&with_paths(EventKind::Create(notify::event::CreateKind::File))).len(),
            1
        );
        assert_eq!(
            paths_from_event(&with_paths(EventKind::Modify(ModifyKind::Name(RenameMode::To)))).len(),
            1
        );
        assert_eq!(
            paths_from_event(&with_paths(EventKind::Remove(notify::event::RemoveKind::File))).len(),
            0
        );
    }
}
