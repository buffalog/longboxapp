# Phase A.8 Interlude: Library Tidy

## Goal

Reconcile LongBox's catalog state with disk reality in both directions, and add scheduled scanning so reconciliation stays continuous.

The phase is inserted between A.8 Step 7 (committed: `0109ed5`) and A.8 Step 8 (release calendar UI). A.8 resumes from Step 8 after this phase closes.

## Why pull this forward

Two real friction points surfaced during Phase A.8 Step 7 verification that the current scan model doesn't address:

1. **Phantom catalog entries.** 28 of 51 tracked series currently have zero owned files. Series records persist independent of file state — when the user removes a series folder from disk, LongBox keeps the series record forever. The user is forced to delete twice (once on disk, once via the UI).

2. **Untracked catalog gap.** The user's Comics volume has 652 series folders; LongBox tracks 51. The 601 untracked series are invisible to the catalog and the pull engine. The Add-series workflow is manual one-at-a-time via CV lookup — onerous for a real library.

3. **Scheduled scan absence.** Scans are currently manual-trigger only. The pull engine's candidate enumeration (A.8 Step 6) assumes a recent scan; without scheduled scans, the catalog drifts stale between manual runs and the pull engine can miss state changes.

These three concerns are tightly coupled — Library Tidy is catalog/disk reconciliation; scheduled scan is the engine that keeps reconciliation continuous.

## Phase scope

Three concerns:

1. **Phantom reconciliation** — catalog tracks series with zero files on disk.
2. **Untracked reconciliation** — disk has series folders LongBox doesn't track.
3. **Scheduled scanning** — keep reconciliation continuous; pull engine correctness depends on fresh catalog.

## Phase kickoff resolutions

### 1. Crate structure

- Scanner additions extend `longbox-scanner` (existing crate).
- Reconciliation routes extend `longbox-web` (new module `routes/reconcile.rs`).
- Scheduled scan task lives in a new `longbox-scan-scheduler` crate, mirroring the `longbox-pull` pattern: `start()` setup-and-return, in-process tokio task, no external cron.

### 2. Series identity for untracked folder discovery

The scanner walks disk and identifies "series-shaped folders" — **top-level** subfolders of `LIBRARY_ROOT_PATH`. A top-level folder is a *discovered (untracked) folder* when it contains ≥1 CBZ file **and no CBZ in it resolved to a tracked series during the scan**. The signal is match-result-based, not folder-name-pattern-based: a tracked series's folder is recognised by its files resolving to that series. (A tracked series whose folder holds only unmatchable files is a rare false positive — the user dismisses it.) Folder name still carries series identity for the user's review per the "series launch year is identity" principle (e.g., `Wolverine (1982)`, `Amazing Spider-Man (1963)`) — it is what gets stored and shown for CV resolution.

**Known limitation — publisher-grouped layouts.** Discovery assumes top-level subfolders *are* series folders (`Library/Wolverine (1982)/…`). A library with a publisher tier (`Library/Marvel/Wolverine (1982)/…`) is unsupported: the top-level folder is then the publisher, real series folders sit one level deeper, and detection collapses every series under the publisher folder. Out of scope for Library Tidy; revisit only if a real publisher-grouped library surfaces.

**Do not auto-resolve to CV.** Folder-name-to-CV mapping requires user disambiguation (multiple CV volumes can match the same title). Store discovered folders in a `discovered_folders` table for user review:

```
folder_name      TEXT UNIQUE NOT NULL
first_seen_at    TIMESTAMP NOT NULL
last_seen_at     TIMESTAMP NOT NULL
dismissed_at     TIMESTAMP NULL
file_count       INTEGER NOT NULL DEFAULT 0
```

`first_seen_at` is the user's accidental discovery timeline; `last_seen_at` is updated every scan that re-detects the folder; `dismissed_at` is set when the user explicitly dismisses a discovery (they don't want to track this series).

### 3. Phantom series definition — two flavors

**Hard definition (transition signal):** series with zero matched files in current scan AND `>0` matched files in prior scan. Strongest possible user-deletion signal — almost certainly the user just deleted a folder. Triggers the proactive reconciliation prompt.

**Soft definition (steady-state):** series currently zero-owned. Listed in tidy view without action prompt — user can review at their leisure. Handles the existing phantom backlog (28 series that were already zero-ownership before LongBox started detecting transitions).

Schema: `series.last_matched_count INT DEFAULT 0` — the last full scan's matched count, updated at the end of each scan. There is **no** consecutive-scan counter. "Steady-state" is therefore the operational definition "currently zero-owned"; "transition" is "currently zero-owned AND `last_matched_count > 0`". The repo returns all zero-owned series with their `last_matched_count`; the steady-state/transition split is a **route-layer** partition, not a repo-layer one.

### 4. Scheduled scan time

- `SCAN_SCHEDULE_TIME` env var (default `03:00`, UTC).
- Runs **before** the pull sweep at `05:00` UTC — catalog is fresh when the pull engine runs.
- Same env-var + tokio task pattern as `PULL_SCHEDULE_TIME` (A.8 Step 6).
- UTC-only (same `OffsetDateTime::now_local()` IndeterminateOffset limitation as Step 6; documented in env var help text).
- Manual scan still available via the UI and the existing `POST /library-roots/:id/scan` endpoint; graceful 409 guard when a scan is already in progress. The scheduled scan participates in the same `scan_status` guard the manual route uses, so the two are mutually exclusive and a scheduled scan shows in the dashboard's live indicator.

### 5. Phase B coexistence

Phase B's file watcher catches drops in `/watch` (real-time). The scheduled scan walks `/library` (daily). Different folders, different concerns. They coexist without overlap.

The scheduled scan does **not** refresh CV metadata for tracked series — that gap stays open and will be addressed (or not) by A.8 Step 8's release calendar / cv_release_cache work.

### 6. Existing phantom backlog

Jeremy already has 28 phantom series in his catalog. The transition-detection mechanism (#3 hard definition) won't catch them — they were already zero-ownership before LongBox started detecting transitions.

The `/library/tidy` view ships two surfaces (Step 5 below):

- **"Recently lost files"** — transition signal (this scan vs. prior scan). Banner-style call to action, per-row review with [Remove from catalog] [Keep] actions.
- **"Zero ownership (all)"** — steady-state list. Lower urgency, bulk-select for cleanup. Handles the existing backlog.

## Step structure

Per-step kickoffs run before each step (5–10 questions each), per the A.8 precedent. Each step lands as a single commit.

### Step 1 — Schema + repos

- Migration: add `series.last_matched_count INT DEFAULT 0`, backfilled from each series' current owned-file count so transition detection is live from the first post-migration scan.
- Create `discovered_folders` table per #2.
- Repo: `series_repo` gains `update_last_matched_count` and `list_phantoms` — the latter returns every zero-owned series with its `last_matched_count` in a localized `PhantomSeries` struct (the new column is **not** added to the widely-used `SeriesRow`). The steady-state/transition split is partitioned by the Step 4 route, not by separate repo methods.
- Repo: new `discovered_folders_repo` with `upsert` / `list` / `dismiss`. A standalone `insert` and `delete` are omitted — `upsert` covers new-folder insertion, and no Step 1–6 caller deletes a row; either is added later if a consumer appears.
- Regenerate `.sqlx/` offline cache via `cargo sqlx prepare --workspace -- --all-targets`.
- Standard schema step shape (mirrors A.8 Step 3).

### Step 2 — Scanner additions

- During full scan: track per-series matched count; update at scan end via `update_last_matched_count`.
- Detect series-shaped folders (top-level subfolders of `/library` containing CBZ files that don't resolve to any tracked series).
- Upsert into `discovered_folders` via the new repo: insert if new, update `last_seen_at` + `file_count` if existing and not dismissed, skip if dismissed.
- Within-step DB writes via Step 1's repos.

### Step 3 — Scheduled scan crate

- New `longbox-scan-scheduler` workspace crate — a generic daily scheduler: `start(config, scan_fn)` spawns the tokio task and returns `ScanSchedulerHandle`. The crate does not depend on `longbox-scanner`/`longbox-web` (the `scan_status` types it would need live in `longbox-web`, which would be a dependency cycle); the scan-with-status logic is a closure built in `bootstrap.rs` and handed to the scheduler.
- `SCAN_SCHEDULE_TIME` env var read from `AppConfig` (default `03:00` UTC).
- The bootstrap closure runs the same `scan_status`-guarded full scan the manual route uses.
- 409 guard is the existing `scan_status` check in `routes/scan.rs` plus the scanner's `scan_lock` — the closure participates in `scan_status`, so no new guard and no route changes.
- `bootstrap.rs` wiring; `AppState` gains the handle.
- Ships its own tests: `duration_until_next` units + a `tokio::time`-mocked test that the scheduler fires the closure at the configured time.

### Step 4 — Reconciliation routes

New module `routes/reconcile.rs`:

- `GET /api/reconcile/phantoms` → returns `{ with_transition: [...], all_zero_owned: [...] }`. Both lists are series with metadata + last_matched_count.
- `GET /api/reconcile/untracked` → list non-dismissed discovered folders.
- `POST /api/reconcile/add` → body `{ folders: [{ folder_name, cv_id }] }`; per-row delegates to the existing add-series workflow (CV fetch + catalog upsert + dismiss the discovered_folders row).
- `POST /api/reconcile/dismiss` → body `{ folder_names: [...] }`; bulk-mark `dismissed_at`.
- `DELETE /api/reconcile/phantom/:series_id` → single series delete (cascade per existing delete-series semantics).
- `POST /api/reconcile/phantoms/bulk` → body `{ series_ids: [...] }`; bulk delete.
- `POST /api/reconcile/phantom/:series_id/keep` → reset `last_matched_count` to 0, demoting a transition phantom to the steady-state list. **(Brief amendment, folded into the Step 5 commit: Step 4's original route enumeration omitted this endpoint even though Step 1's `update_last_matched_count` doc-comment names the Step 5 "Keep" action as its consumer. Implemented in Step 5 alongside the UI that drives it.)**

Typed 409 / 404 surfacing per A.8 convention.

### Step 5 — `/library/tidy` UI

Two-section page with sub-sections for the phantom side:

**Phantom series**
- **Subsection 1: "Recently lost files"** (transition signal) — banner-call-to-action style. Each row shows cover, title, previous owned count. Per-row [Remove from catalog] [Keep] (Keep resets `last_matched_count` to 0 so it falls to subsection 2).
- **Subsection 2: "Zero ownership"** (steady-state backlog) — list with bulk-select + bulk-remove. Handles the 28 existing phantoms.

**Untracked folders**
- List of discovered folders with `file_count`.
- Per-row: [Add to LongBox] (opens CV search modal pre-populated with folder name; reuse the existing "Search ComicVine" modal pattern from `/files` folder cards), [Dismiss].
- Bulk-select for [Dismiss all selected].
- No bulk-add — CV resolution requires per-folder user disambiguation.

Temporary flat `/library/tidy` nav link (with `// TODO Step 12 (A.8): fold into nav restructure` comment) per the A.8 Step 7 precedent.

### Step 6 — Dashboard reconciliation banner

- `GET /api/reconcile/counts` → `{ phantoms_with_transition, untracked_folders }` — a lightweight counts endpoint (reuses the list repos and counts in Rust; no new SQL, no `.sqlx` entry) so the dashboard never pulls the full phantom/untracked lists onto the landing page.
- `ReconciliationBanner.svelte` rendered at the top of the dashboard (route `/`) when `phantoms_with_transition > 0` OR `untracked_folders > 0`. The counts are fetched failure-tolerantly — a failure means no banner, never a page error.
- Copy assembled conditionally — one sentence per non-zero count, singular/plural-aware: "*N series lost their files. M untracked folders detected.*" + a "Review →" link to `/library/tidy`.
- **Dismissal — localStorage count-signature model.** Dismissing stores the current `transition:untracked` count signature in `localStorage`. The banner stays hidden while the live counts still match the stored signature, and reappears when either count changes. Dismiss is cosmetic — it never clears the underlying reconciliation state. (Brief amendment, folded into the Step 6 commit: this replaces the original draft's ambiguous "current session / localStorage flag / returns next visit" wording, which was internally inconsistent — the count-signature model is the concrete behavior.)

### Step 7 — Tests

- Backend route tests in `api_tests.rs`: full CRUD coverage, bulk operations, dismissal idempotency, 404s for unknown series/folders.
- Scanner integration tests: transition detection (mocked file state changes between two scans), discovered folder upserts (new / existing / dismissed paths).
- (The `longbox-scan-scheduler` crate's own unit + `tokio::time`-mocked tests ship with Step 3; Step 7 keeps only broader cross-component integration coverage.)
- Frontend `api/*.ts` unit tests + `@testing-library/svelte` component tests for `/library/tidy` (per A.8 Step 5/7 component-test pattern, IndexerSettings.test.ts as template).

### Step 8 — End-to-end verification (optional, may fold into Step 7)

Two scenarios as integration tests against a test DB + temp library:

1. Drop series folder → scheduled scan walks → folder discovered → user adds via UI → catalog populated, folder dismissed.
2. Delete series folder → scheduled scan walks → series transitions to phantom → user removes via UI → series gone from catalog.

May be folded into Step 7 if not warranted as its own step.

## Brief tracking

This brief committed at phase kickoff per the A.7 `c968aec` / A.8 `0c921de` precedent. Filename: `longbox-library-tidy-prompt.md`.

## Resume point

After this phase closes: **A.8 Step 8** (release calendar UI). Step 8's kickoff already surfaced relevant constraints (publisher deferred, compound add-to-pull-list, no auto-catalog-refresh) — those resolutions stay locked when we resume.

Library Tidy may build the underlying "add series by cv_id" primitive (Step 4 above), which Step 8 then reuses for its per-row "Add to pull list" action. Sequencing favors Library Tidy first.

## Phase exit criteria

- Phantom series with transition signals surface in `/library/tidy` after a scan that detects them.
- Existing phantom backlog (28 series) reviewable in the "Zero ownership" subsection.
- Untracked folders surface in `/library/tidy` and can be added via CV search modal or dismissed.
- Scheduled scan runs daily at `SCAN_SCHEDULE_TIME`; falls back to manual trigger when needed.
- Dashboard banner surfaces non-zero reconciliation state, clicks through to `/library/tidy`.
- Test coverage per Step 7.
- A.8 Step 8 can resume cleanly with `add-series-by-cv_id` primitive available.
