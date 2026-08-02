use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, Semaphore};

use crate::config::AppConfig;

#[derive(Clone)]
pub struct AppState {
    pub db: longbox_db::Pool,
    pub cv: Arc<longbox_comicvine::ComicVineClient>,
    /// Unthrottled CV client for interactive/user-initiated requests (e.g.,
    /// Library Tidy manual cv-id picks). Rate-limited to 3600/hr (practically
    /// unlimited for one-at-a-time user actions) with a short 3s wait-for-slot
    /// so the handler fails fast rather than blocking for 30-60s.
    pub cv_direct: Arc<longbox_comicvine::ComicVineClient>,
    /// Metron forward-calendar client (Item A v2). `None` when the
    /// `metron_enabled` settings row is `false` OR when credentials are
    /// missing/invalid (degraded boot path). Forward-week calendar
    /// requests short-circuit to "no releases" when this is `None`;
    /// current-week requests are unaffected.
    pub metron: Option<Arc<longbox_metron::MetronClient>>,
    pub scanner: Arc<longbox_scanner::Scanner>,
    pub config: Arc<AppConfig>,
    /// Serialises file deletions across the two features that perform
    /// them (Library Tidy's resolve, Library Integrity's per-group
    /// delete).
    ///
    /// Both check "at least one other verified copy survives" and then
    /// delete. Without a lock those two steps interleave: two requests
    /// naming different members of a two-file group each observe the
    /// other as the survivor, both pass, and the group empties —
    /// defeating the one guard the whole feature rests on. The window
    /// is not small; the guard stats every sibling between reading and
    /// writing.
    ///
    /// **Ceiling, stated rather than implied:** this serialises deletes
    /// *within this process only*. A second LongBox process against the
    /// same database, or a hand-run `sqlite3`, is unaffected. Deletes
    /// here are human-paced and one-at-a-time by design, so a process-
    /// wide lock costs nothing and a heavier mechanism would buy no
    /// real protection.
    ///
    /// The test harness cannot demonstrate the race it prevents:
    /// `:memory:` pools are pinned to `max_connections(1)` while
    /// production runs sqlx's default of 10, so the interleaving is
    /// structurally impossible in tests. Correctness here is argued
    /// from the critical section, not shown by the suite.
    pub delete_lock: Arc<tokio::sync::Mutex<()>>,
    pub scan_status: Arc<RwLock<ScanStatus>>,
    /// Live state of the content-analysis pass (Library Integrity). In-memory
    /// only, like `scan_status` — there is no analysis history to render.
    pub analyze_status: Arc<RwLock<AnalyzeStatus>>,
    /// The library root row id created or matched at bootstrap. Phase A
    /// has exactly one; cached here so handlers don't re-query.
    pub library_root_id: i64,
    /// Phase B's pending-intervention cache. Read by the dashboard
    /// counter and the `/files/pending-intervention` list view; written
    /// by the postprocess consumer task and the notify watcher. Always
    /// present (empty when Phase B is disabled or no files are stuck)
    /// so handlers don't need to special-case the absent state.
    pub pending_cache: Arc<longbox_postprocess::PendingInterventionsCache>,
    /// Handle to the Phase A.8 pull engine. Always present — the
    /// scheduler task runs unconditionally; the `/pull/check` route
    /// uses this to trigger an immediate sweep.
    pub pull: longbox_pull::PullHandle,
    /// On-demand single-series search tracker. Independent of `pull`
    /// (which guards the daily scheduler); guards `/pull/search/:id`
    /// and the auto-fire-on-subscribe hook. Per-series concurrency:
    /// two searches for distinct series run in parallel, two for the
    /// same series return 409.
    pub pull_search: longbox_pull::PullSearchHandle,
    /// Handle to the Library Tidy scheduled-scan timer. Always present;
    /// the daily scan runs through the same `scan_status` guard as the
    /// manual scan route.
    pub scan_scheduler: longbox_scan_scheduler::ScanSchedulerHandle,
    /// Handle to the A.9 Step 6c.2 CV enrichment worker. Always present;
    /// the worker enforces its own startup migration check and refuses
    /// to attempt anything if the 6c.1 columns are missing, so this
    /// handle is safe to expose regardless of schema state.
    pub enrichment: longbox_cv_enrichment::EnrichmentHandle,
    /// Process boot time. Captured once at bootstrap and never
    /// mutated; `GET /api/health` subtracts from `now_utc()` to derive
    /// `uptime_seconds`. `OffsetDateTime` (UTC) rather than
    /// `Instant` because the latter has no defined epoch — we want a
    /// timestamp we can serialize/compare; durations come from
    /// arithmetic, not the type.
    pub start_time: time::OffsetDateTime,
    /// Bounds background pull-list resolutions that need a ComicVine
    /// fetch (not-yet-local volumes added from the Release Calendar) to a
    /// few at a time, so a bulk add never stampedes the rate limiter's
    /// burst and trips `max_wait_for_slot` (the silent-`RateLimited`
    /// partial-failure bug the old 10-wide fan-out caused). Set to 3 in
    /// `bootstrap` — low enough to stay under the burst ceiling, high
    /// enough that a light add isn't head-of-line-blocked behind a
    /// back-catalog giant mid-fetch. ponytail: a fixed small bound, not
    /// adaptive; revisit only if CV's limits or the cap change.
    pub cv_resolve_gate: Arc<Semaphore>,
    /// Live outcomes of those background resolutions, keyed by the
    /// frontend row discriminator (`cv:{id}` / `metron:{id}`). The
    /// calendar polls `GET /api/releases/calendar/pull/status` after a
    /// bulk add; terminal entries are pruned read-once. ponytail:
    /// in-memory + single-user read-once semantics — lost on restart,
    /// which is fine for a transient UX cue, not a durable record.
    pub subscribe_status: Arc<RwLock<SubscribeStatus>>,
}

/// One background pull-list resolution's current state. `status` is
/// `resolving` until the background task lands a terminal `added` /
/// `already_on_list` / `failed`. `series_id` and `error` are populated
/// on the terminal write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeOutcome {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SubscribeOutcome {
    /// Whether this outcome is terminal (anything other than `resolving`).
    pub fn is_terminal(&self) -> bool {
        self.status != "resolving"
    }
}

/// In-memory map of in-flight + recently-terminal background pull-list
/// resolutions, keyed by the frontend row discriminator. Serializes to a
/// JSON object under `items` for the status poll.
#[derive(Debug, Default, Serialize)]
pub struct SubscribeStatus {
    pub items: HashMap<String, SubscribeOutcome>,
}

/// In-memory mid-scan status. The "current" pill in the UI reads from
/// this; persisted scan history lives in the `scan_runs` table (read via
/// `scan_run_repo::list_recent`). Per Task C: in-memory state is only for
/// the live in-flight indicator, never for history rendering.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScanStatus {
    pub current: Option<CurrentScan>,
}

/// State of the Library Integrity content-analysis pass.
///
/// The analysis reads files and writes digests; this tracks whether it is
/// in flight and what the last pass did. Nothing here is persisted — a
/// restart mid-analysis simply means the next pass re-hashes whatever has no
/// fresh digest, which is the same self-correcting behaviour that governs
/// staleness.
#[derive(Debug, Default, Clone, Serialize)]
pub struct AnalyzeStatus {
    pub running: bool,
    #[serde(with = "time::serde::iso8601::option")]
    pub started_at: Option<time::OffsetDateTime>,
    #[serde(with = "time::serde::iso8601::option")]
    pub finished_at: Option<time::OffsetDateTime>,
    /// Stats from the most recent completed pass.
    pub last: Option<crate::content_hash::HashStats>,
    /// Set when the last pass failed outright (a DB error reading the
    /// candidate set). Per-file failures are counted in `last.failed`
    /// instead — one bad archive never sinks a pass.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentScan {
    pub scan_id: String,
    pub library_root_id: i64,
    pub kind: ScanKind,
    #[serde(with = "time::serde::iso8601")]
    pub started_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanKind {
    Full,
    RescanUnmatched,
    RematchForSeries,
}
