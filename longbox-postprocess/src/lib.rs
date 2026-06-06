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

use longbox_db::settings_repo;
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

/// Read the live `match_confidence_threshold` setting that Phase B's
/// owned-classification consults. Same key, same fallback contract as
/// the scanner's `Scanner::load_match_threshold` — these two phases
/// agree on what "confident enough to claim" means by reading the
/// same row. Falls back to the compiled `DEFAULT_MATCH_THRESHOLD` when
/// the row is absent or unparseable, but never silently uses a stale
/// boot-time value.
async fn load_owned_threshold(db: &longbox_db::Pool) -> Result<f64> {
    let value = settings_repo::get_or_default(
        db,
        settings_repo::KEY_MATCH_CONFIDENCE_THRESHOLD,
        longbox_core::DEFAULT_MATCH_THRESHOLD,
    )
    .await?;
    Ok(value)
}

/// Outcome tally for [`sweep_now`]. Mirrors the [`processor::Outcome`]
/// enum collapsed to per-bucket counts. Serialized as the response body
/// of `POST /api/postprocess/trigger` so the frontend can render a
/// "processed N, skipped K, conflicts M" toast off one round-trip.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SweepSummary {
    /// Matched + moved + catalogued as owned.
    pub processed: usize,
    /// No usable hint, no catalog match, or sub-threshold match — the
    /// source stays in the watch folder. WARN log carries the reason.
    /// Replaces the old `unsorted` bucket (which moved files to a
    /// `_unsorted/` parking lot) per Jeremy's directive.
    pub skipped: usize,
    /// Library already owned canonical bytes; the source was auto-removed
    /// from the watch folder (see `processor::cleanup_conflict_source`).
    pub conflicts: usize,
    /// Stage-specific failure during processing (ComicInfo write, move,
    /// mid-flight disappearance). Counter for the operator; details
    /// land in the WARN log under `phase_b.sweep_now_error` or
    /// `phase_b.failed`.
    pub failed: usize,
}

/// On-demand drain of the watch folder. Same producer pipeline as the
/// background `consumer_task` (walk → [`skip::should_skip`] →
/// [`processor::process_one`] → cache update), but called inline by
/// the user via `POST /api/postprocess/trigger` so it returns a
/// summary rather than logging-and-forgetting.
///
/// Running concurrently with the watcher is safe but wasteful: both
/// paths may pick up the same file. The processor's metadata recheck
/// + same-file conflict cleanup absorb the race — the second arrival
/// either reports `Failed` (source gone) or `Conflict` (target landed
/// from the other path). Neither outcome corrupts state.
pub async fn sweep_now(
    library_root: &Path,
    library_root_id: i64,
    watch_path: &Path,
    db: longbox_db::Pool,
    cache: Arc<PendingInterventionsCache>,
) -> Result<SweepSummary> {
    // Fail fast on a missing/unreadable watch folder so the route can
    // turn this into a clean 400 rather than letting walkdir crawl
    // silently across whatever-the-default is.
    std::fs::read_dir(watch_path).map_err(|source| PostprocessError::WatchPathUnreadable {
        path: watch_path.to_path_buf(),
        source,
    })?;

    // Read the live owned threshold once per sweep — same cadence as
    // the scanner's `Scanner::load_match_threshold` (per scan run).
    // The DB settings row is the single source of truth; the constant
    // fallback only applies when the row is absent/unparseable
    // (initial-boot state, never after the first save).
    let owned_threshold = load_owned_threshold(&db).await?;

    let watch_path_buf = watch_path.to_path_buf();
    let walk_root = watch_path_buf.clone();
    let candidates = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(&walk_root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.into_path();
            if skip::should_skip(&path).is_none() {
                out.push(path);
            }
        }
        out
    })
    .await
    .map_err(|e| PostprocessError::Io(std::io::Error::other(format!("join: {e}"))))?;

    let total = candidates.len();
    tracing::info!(
        target: "longbox_postprocess",
        candidates = total,
        root = %watch_path_buf.display(),
        "phase_b.sweep_now_started"
    );

    let mut summary = SweepSummary::default();
    for path in candidates {
        match processor::process_one(
            &path,
            library_root,
            library_root_id,
            &db,
            owned_threshold,
        )
        .await
        {
            Ok(outcome) => {
                use processor::Outcome;
                match &outcome {
                    Outcome::Imported { .. } => summary.processed += 1,
                    Outcome::Skipped { .. } => summary.skipped += 1,
                    Outcome::Conflict { .. } => summary.conflicts += 1,
                    Outcome::Failed { .. } => summary.failed += 1,
                }
                apply_outcome_to_cache(&cache, &path, outcome);
            }
            Err(e) => {
                summary.failed += 1;
                tracing::warn!(
                    target: "longbox_postprocess",
                    path = %path.display(),
                    err = %e,
                    "phase_b.sweep_now_error"
                );
            }
        }
    }

    // SAB drops one job folder per download (`/watch/Saga 001/`); once
    // Phase B moves the CBZ out, the folder is empty. The per-file
    // `cleanup_empty_parent` covers Imported outcomes during the loop,
    // but Skipped/Conflict/Failed don't trigger it, AND a folder
    // becoming empty AFTER its last file imports earlier in the sweep
    // wouldn't be visible to the per-file pass. End-of-sweep walk
    // catches both cases. Operates ONLY on directly-empty directories
    // (zero entries) — leftover `.par2`/`.nfo` files are preserved.
    let dirs_removed = cleanup_empty_watch_dirs(watch_path);

    tracing::info!(
        target: "longbox_postprocess",
        processed = summary.processed,
        skipped = summary.skipped,
        conflicts = summary.conflicts,
        failed = summary.failed,
        dirs_removed,
        "phase_b.sweep_now_complete"
    );
    Ok(summary)
}

/// Walk `watch_path` bottom-up and remove any subdirectory that has
/// zero entries. Returns the count removed for telemetry. Never
/// touches the watch root itself; never recurses into something that
/// isn't a directory; silently skips entries that fail to canonicalize
/// or that another process removed between the walk and the rmdir
/// (best-effort, race-tolerant).
///
/// Bottom-up ordering is load-bearing: a folder that contains only
/// empty subfolders becomes itself empty after the inner ones get
/// removed, so we must process leaves first. `walkdir::contents_first`
/// provides that order natively.
fn cleanup_empty_watch_dirs(watch_path: &Path) -> usize {
    let canonical_root = match std::fs::canonicalize(watch_path) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(
                target: "longbox_postprocess",
                root = %watch_path.display(),
                err = %e,
                "phase_b.cleanup_empty_dirs_skipped (root canonicalize failed)"
            );
            return 0;
        }
    };
    let mut removed = 0usize;
    for entry in walkdir::WalkDir::new(watch_path)
        .follow_links(false)
        .contents_first(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        let canonical = match std::fs::canonicalize(entry.path()) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Never remove the watch root itself. Same guard as
        // `cleanup_empty_parent` — losing it would silently break the
        // pipeline by removing the directory the watcher is observing.
        if canonical == canonical_root {
            continue;
        }
        let entry_count = match std::fs::read_dir(&canonical) {
            Ok(it) => it.count(),
            Err(_) => continue,
        };
        if entry_count != 0 {
            continue;
        }
        match std::fs::remove_dir(&canonical) {
            Ok(()) => {
                removed += 1;
                tracing::debug!(
                    target: "longbox_postprocess",
                    dir = %canonical.display(),
                    "phase_b.watch_dir_cleaned (end-of-sweep)"
                );
            }
            Err(e) => {
                tracing::debug!(
                    target: "longbox_postprocess",
                    dir = %canonical.display(),
                    err = %e,
                    "phase_b.watch_dir_cleanup_failed (end-of-sweep)"
                );
            }
        }
    }
    removed
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
        // Per-message threshold read so a Settings UI change takes
        // effect from the very next watched file — same "live tunable"
        // promise the scanner makes. SQLite-local read, microseconds.
        let owned_threshold = match load_owned_threshold(&db).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    target: "longbox_postprocess",
                    err = %e,
                    "phase_b.threshold_load_failed (falling back to compiled default)"
                );
                longbox_core::DEFAULT_MATCH_THRESHOLD
            }
        };
        match processor::process_one(&path, &library_root, library_root_id, &db, owned_threshold)
            .await
        {
            Ok(outcome) => {
                // Imported is the only outcome that removes the source
                // from /watch/. Skipped intentionally leaves the file
                // in place (per Jeremy's directive — /watch/ is the
                // holding pen), so the parent dir is still non-empty
                // and we must NOT try to remove it.
                if matches!(&outcome, processor::Outcome::Imported { .. }) {
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
        Outcome::Imported { .. } | Outcome::Skipped { .. } | Outcome::Conflict { .. } => {
            // Three non-Failed outcomes, all evict any prior pending-
            // intervention entry:
            //   - Imported: moved + catalogued, nothing to intervene on.
            //   - Skipped: stays in /watch/; the file is visible in
            //     the watch folder and `phase_b.skipped` WARN logs the
            //     reason. We deliberately do NOT push the operator
            //     a dashboard task — the holding pen IS the surface.
            //   - Conflict: cleanup_conflict_source removed the source
            //     (or logged conflict_cleanup_failed); library has
            //     canonical bytes either way.
            cache.remove_by_source_path(source);
        }
        Outcome::Failed {
            reason,
            target,
            size,
        } => {
            // Race-straggler guard: if the source isn't on disk by the
            // time the outcome lands here, another path (watcher
            // consumer or a concurrent sweep_now walker) already
            // handled the file — typically by importing it before this
            // run got there, and the failure we're holding is the
            // loser's ENOENT during read or move. Pushing a Failed
            // entry now would surface a phantom intervention for a
            // file the operator can't act on (Finder-link 404s, the
            // dashboard claims pending work that's already done).
            // Evict any stale entry too so a previously-stuck row from
            // a different race clears at the same time.
            //
            // The guard is correctness-preserving for the genuine
            // failure case: a Failed outcome that actually has a
            // source on disk (stage-1 rewrite failed, stage-2 move
            // failed mid-flight with bytes still at the source) sees
            // `source.exists() == true` and gets pushed exactly as
            // before. The only behavior change is in the no-source
            // case, which by definition has nothing to intervene on.
            if !source.exists() {
                cache.remove_by_source_path(source);
                return;
            }
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
    fn apply_outcome_evicts_on_conflict() {
        // Phase B now removes the source on conflict
        // (`processor::cleanup_conflict_source`), so the cache must
        // treat Conflict as resolved — same disposition as Imported /
        // Unsorted. A previously-stuck entry for the same source path
        // is evicted; a fresh Conflict adds nothing.
        use processor::Outcome;
        let cache = PendingInterventionsCache::new();
        let source = PathBuf::from("/watch/saga 001.cbz");
        // Pre-seed a stuck entry directly (the prior cache-push path
        // is gone, so we can't seed via apply_outcome_to_cache).
        cache.push(PendingIntervention {
            source_path: source.clone(),
            target_path: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
            reason: InterventionReason::Conflict,
            size: 4096,
            last_attempt: time::OffsetDateTime::UNIX_EPOCH,
        });
        assert_eq!(cache.len(), 1);
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Conflict {
                target: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
                size: 4096,
            },
        );
        assert!(cache.is_empty(), "Conflict must evict, not cache");
    }

    #[test]
    fn apply_outcome_caches_failed_when_source_is_on_disk() {
        // Genuine failure: source bytes are still in /watch/ because
        // the import broke mid-flight (stage-1 rewrite, stage-2 move).
        // The cache MUST push so the operator sees the stuck file.
        use processor::Outcome;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("saga 001.cbz");
        std::fs::write(&source, b"placeholder bytes").unwrap();

        let cache = PendingInterventionsCache::new();
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
        assert_eq!(snap.len(), 1, "genuine Failed with on-disk source must push");
        assert!(matches!(
            snap[0].reason,
            InterventionReason::MoveFailed(ref m) if m == "EXDEV"
        ));
    }

    #[test]
    fn apply_outcome_evicts_failed_when_source_already_imported_by_race_winner() {
        // The fix for the watcher/sweep race observed in production:
        // when 165 files were bulk-mv'd into /watch/, the notify
        // watcher and a concurrent sweep_now both picked up each file.
        // The watcher consumer won most races and successfully
        // imported; sweep_now lost and got ENOENT during its rewrite/
        // move stage, surfacing as `Outcome::Failed { MoveFailed }`.
        //
        // Pre-fix: those 92 lost-race Failures pushed phantom
        // pending-intervention entries for files that were ALREADY in
        // the library — the dashboard's Finder link would 404 because
        // the source path no longer existed.
        //
        // Fix: when Failed lands and source.exists() == false, treat
        // it as "another path already handled this; nothing to
        // intervene on" — evict any stale entry (could be from yet
        // another race) and bail without pushing.
        use processor::Outcome;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("imported-by-watcher.cbz");
        assert!(!source.exists(), "fixture: source is intentionally absent");

        let cache = PendingInterventionsCache::new();
        // Simulate a stale entry from a prior race round.
        cache.push(PendingIntervention {
            source_path: source.clone(),
            target_path: PathBuf::from("/lib/stale.cbz"),
            reason: InterventionReason::MoveFailed("old race".into()),
            size: 1,
            last_attempt: time::OffsetDateTime::UNIX_EPOCH,
        });
        assert_eq!(cache.len(), 1);

        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Failed {
                reason: InterventionReason::MoveFailed(
                    "io error: No such file or directory (os error 2)".into(),
                ),
                target: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
                size: 4096,
            },
        );
        assert!(
            cache.is_empty(),
            "race-straggler Failed must NOT push when source is missing; \
             and any stale prior entry must be cleared at the same time"
        );
    }

    #[test]
    fn apply_outcome_evicts_on_imported() {
        use processor::Outcome;
        let cache = PendingInterventionsCache::new();
        let source = PathBuf::from("/watch/saga 001.cbz");
        // Pre-seed a stuck Failed entry directly (the race-guard rule
        // means we can't seed via apply_outcome_to_cache + Failed on a
        // synthetic path), then run a successful import for the same
        // source — self-healing.
        cache.push(PendingIntervention {
            source_path: source.clone(),
            target_path: PathBuf::from("/lib/Saga (2012)/Saga (2012) 001.cbz"),
            reason: InterventionReason::MoveFailed("EXDEV".into()),
            size: 4096,
            last_attempt: time::OffsetDateTime::UNIX_EPOCH,
        });
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
    fn apply_outcome_evicts_on_skipped() {
        // Skipped files stay in /watch/; the WARN log is the operator
        // surface, NOT a pending-intervention entry. A previously-stuck
        // Failed entry for the same source path gets cleared when the
        // next pass classifies the file as Skipped (e.g. the user
        // added the missing series in the meantime, or the file just
        // never matches — either way it's no longer "stuck", it's
        // "deliberately left where the operator can see it").
        use processor::Outcome;
        let cache = PendingInterventionsCache::new();
        let source = PathBuf::from("/watch/mystery.cbz");
        // Direct seed — synthetic path won't push via Failed now that
        // the race-guard checks source.exists().
        cache.push(PendingIntervention {
            source_path: source.clone(),
            target_path: PathBuf::from("/lib/Unknown.cbz"),
            reason: InterventionReason::ComicInfoWriteFailed("permission".into()),
            size: 1,
            last_attempt: time::OffsetDateTime::UNIX_EPOCH,
        });
        apply_outcome_to_cache(
            &cache,
            &source,
            Outcome::Skipped {
                reason: "no catalog match for series hint \"Unknown\"".into(),
            },
        );
        assert!(cache.is_empty(), "Skipped must evict any stale entry");
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
    fn cleanup_empty_watch_dirs_removes_all_empty_subdirs_at_end_of_sweep() {
        // End-of-sweep helper: walks the watch root bottom-up and
        // removes every empty subdirectory. Covers the Skipped /
        // Conflict / Failed cases where the per-file
        // `cleanup_empty_parent` doesn't fire, AND the case where a
        // dir becomes empty later in the sweep due to other-path
        // removals.
        let watch = TempDir::new().unwrap();
        // Three empty job folders (Skipped/Conflict/Failed leftovers).
        let empty_a = watch.path().join("empty-job-a");
        let empty_b = watch.path().join("empty-job-b");
        let empty_nested = watch.path().join("nested-empty/inner");
        std::fs::create_dir(&empty_a).unwrap();
        std::fs::create_dir(&empty_b).unwrap();
        std::fs::create_dir_all(&empty_nested).unwrap();
        // One non-empty job folder (operator left a .par2 behind, or
        // a Failed outcome's source is still there).
        let nonempty = watch.path().join("nonempty-job");
        std::fs::create_dir(&nonempty).unwrap();
        std::fs::write(nonempty.join("leftover.par2"), b"data").unwrap();

        let removed = cleanup_empty_watch_dirs(watch.path());

        assert!(!empty_a.exists(), "empty-job-a must be removed");
        assert!(!empty_b.exists(), "empty-job-b must be removed");
        assert!(
            !empty_nested.exists(),
            "nested-empty/inner must be removed"
        );
        assert!(
            !empty_nested.parent().unwrap().exists(),
            "nested-empty parent must be removed (bottom-up cascades)"
        );
        assert!(nonempty.exists(), "non-empty dir must survive");
        assert!(nonempty.join("leftover.par2").exists());
        // The watch root itself must always survive.
        assert!(watch.path().exists());
        assert_eq!(
            removed, 4,
            "two flat + two nested removals expected; got {removed}"
        );
    }

    #[test]
    fn cleanup_empty_watch_dirs_never_removes_the_watch_root() {
        // Same load-bearing safety guard as `cleanup_empty_parent`:
        // even on a brand-new empty watch folder, the root itself
        // must NOT be removed — losing it would silently break the
        // pipeline.
        let watch = TempDir::new().unwrap();
        assert_eq!(std::fs::read_dir(watch.path()).unwrap().count(), 0);

        let removed = cleanup_empty_watch_dirs(watch.path());

        assert_eq!(removed, 0);
        assert!(watch.path().exists(), "watch root must survive");
    }

    #[test]
    fn cleanup_empty_watch_dirs_tolerates_missing_root() {
        // If the watch path itself vanished (mount lost, user
        // intervention), the function must no-op silently rather
        // than panic.
        let removed = cleanup_empty_watch_dirs(std::path::Path::new(
            "/longbox-test-nonexistent-watch-path",
        ));
        assert_eq!(removed, 0);
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
