# LongBox state-of-system snapshot

**Date:** 2026-05-28
**From:** Claude Code (in `/Users/jeremy/Projects/longbox`)
**To:** Jude (claude.ai)
**Purpose:** ground-truth reference Jude can drill into without round-tripping archaeology through Jeremy. Captures topology, crates, the pull engine (priority), reconcile + parser internals, schema, integrations, workflow rules, deferred items, data state, and open architectural questions.

This complements the 2026-05-26 in-flight handoff (`longbox-a9-handoff-to-jude.md`). That one captured the work-in-flight; this one captures the *system*.

---

## 1. System topology

**Single LongBox container** runs the Axum HTTP server, the embedded SvelteKit frontend (rust-embed baked at build time), the scan scheduler, the post-process watcher, and the pull scheduler. All in one process, one binary, one container — no microservices. State lives in two places: SQLite at `/data/longbox.db`, comic archives at `/library`.

```
┌────────────────────────────────────────────────────────────────┐
│  Host: 16" MBP M5 Max / macOS Tahoe 26.4.1                     │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  Colima VM (aarch64, virtiofs)                           │  │
│  │                                                           │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │  Container: longbox                                │  │  │
│  │  │  Image: longbox:latest (Alpine 3.20 runtime)       │  │  │
│  │  │  User: longbox (uid 1000)                          │  │  │
│  │  │  Bind: 0.0.0.0:3000                                │  │  │
│  │  │                                                     │  │  │
│  │  │  Internal tasks:                                   │  │  │
│  │  │   - Axum HTTP server (the binary's entry point)    │  │  │
│  │  │   - Scan scheduler (longbox-scan-scheduler)        │  │  │
│  │  │   - Pull scheduler  (longbox-pull::schedule)       │  │  │
│  │  │   - Post-process watcher (longbox-postprocess)     │  │  │
│  │  │     ├── initial sweep task                         │  │  │
│  │  │     ├── notify::Watcher task                       │  │  │
│  │  │     └── consumer task                              │  │  │
│  │  │                                                     │  │  │
│  │  │  Mounts:                                           │  │  │
│  │  │   - longbox-data:/data  (named volume, SQLite)     │  │  │
│  │  │   - $LIBRARY_PATH:/library  (host /Volumes/Comics) │  │  │
│  │  │   - $DOWNLOAD_WATCH_HOST:/watch                    │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
│  Host port 3000 → container 3000                                │
└────────────────────────────────────────────────────────────────┘
```

### External services LongBox talks to (none colocated)

| Service       | Where it lives                                    | Protocol           | Auth                                 |
| ------------- | ------------------------------------------------- | ------------------ | ------------------------------------ |
| ComicVine     | `comicvine.gamespot.com/api/`                     | REST (JSON)        | `api_key` query param                |
| Prowlarr      | `192.168.1.163:9696/N` (per-indexer-proxy path)   | Newznab XML        | `apikey` query param                 |
| SABnzbd       | `192.168.1.163:8081/api`                          | REST (JSON / text) | `apikey` query param                 |
| Slack webhook | `hooks.slack.com/services/…`                      | POST (block-kit)   | secret in URL                        |

Slack and SABnzbd live on a separate Windows machine on the LAN (`192.168.1.163`). Prowlarr runs alongside SAB. **There is no filesystem bridge** between SAB's complete dir on Windows and the LongBox watch folder on Mac — that's the Scenario-1 smoke blocker as of 2026-05-26.

### Network surface

- **Inbound:** `3000/tcp` on host = LongBox HTTP + UI. No TLS, no auth — bound to all interfaces.
- **Outbound from LongBox:** ComicVine HTTPS, Prowlarr HTTP (LAN), SABnzbd HTTP (LAN), Slack HTTPS. No inbound webhooks (LongBox does not receive callbacks from SAB — it polls).

### Where state lives

- **`/data/longbox.db`** — SQLite, WAL mode. Single source of truth for series, issues, files, pull-list, attempts, indexer/downloader/webhook config, settings, scan history, parser patterns, discovered folders.
- **`/library`** — read-write bind mount of the user's comic library. Phase B post-process moves files in; the scanner walks it; the reconcile path classifies top-level folders as tracked or untracked.
- **`/watch`** — read-write bind mount of a host folder where downloaders land completed NZBs. Phase B watches it for new arrivals.
- **In-memory** — pending-intervention cache (`Arc<PendingInterventionsCache>` in `longbox-postprocess`), per-process. Not persisted; rebuilt on restart from re-detecting stuck files on the initial sweep.

### docker-compose / Dockerfile shape

- **`docker-compose.yml`** — single `longbox` service. Healthcheck hits `/api/health`. Auto-restart `unless-stopped`. Env-driven config (`LIBRARY_PATH`, `DOWNLOAD_WATCH_HOST`, `COMICVINE_API_KEY`, `MATCH_THRESHOLD`, `LOG_LEVEL`).
- **`Dockerfile`** — three stages:
  1. **frontend-builder** — `node:20-alpine` + `pnpm@10`. Builds SvelteKit static bundle to `longbox-web/frontend-dist/`.
  2. **backend-builder** — `rust:1.95-alpine` with musl static link. Compiles `cargo build --release --target aarch64-unknown-linux-musl --package longbox-web`. Uses `SQLX_OFFLINE=true` against the checked-in `.sqlx/` cache, so no DB is needed at build time. `RUSTFLAGS=-C target-feature=+crt-static` for a fully-static binary.
  3. **runtime** — `alpine:3.20` + `ca-certificates`. Just the binary, the `longbox` user, and the `/data` / `/library` mount points.
- Frontend bundle is *baked into the binary* via `rust-embed`. The runtime has no Node, no static-asset server, no NGINX.

### Colima mount fragility (permanent operational hazard)

`/Volumes/Comics` is a virtiofs bind mount of a host volume into the Colima VM. **Any host-side unmount/remount strands the VM on a stale view** until `colima restart` re-syncs. Surfaced first 2026-05-21 (`docker-compose up` failed with `mkdir /Volumes/Comics: file exists`). Codified as workflow rule.

---

## 2. Crate structure

**Workspace:** 12 crates, all `version = "0.0.1"`, edition 2021 (except `longbox-archive` which is 2024 because `unrar-ng` requires it).

| Crate                     | Lines (src) | Purpose                                                                 |
| ------------------------- | ----------- | ----------------------------------------------------------------------- |
| `longbox-core`            | ~700        | Pure domain types + logic. No I/O, no SQL, no HTTP. Owns `ParsingPattern`, `ParsedFilename`, `MatchMethod`, `FileStatus`, `LibraryPath`, `ComicInfo`, matcher, normalizer, similarity. |
| `longbox-archive`         | ~300        | CBZ (ZIP) + CBR (RAR) read; `read_comic_info()` and `read_entries()`. Wraps `zip` and `unrar-ng`. |
| `longbox-db`              | ~3000       | sqlx `Pool`, embedded migrations, repository layer (one repo per table). Re-exports row types. |
| `longbox-comicvine`       | ~600        | ComicVine REST client; 180 req/h rate limit, burst 5, retry policy. Owns the CV-side data model + the catalog projection. |
| `longbox-newznab`         | ~800        | Newznab indexer client. Single-indexer search, cross-indexer `find_release_excluding`, padded/unpadded query strategy, cbz>cbr>unknown release ranking. |
| `longbox-downloader`      | ~700        | `Downloader` trait + SABnzbd + NZBGet impls. `submit() → DownloadHandle`, `status()` → `DownloadStatus`. |
| `longbox-webhooks`        | ~130        | Plain delivery client. Slack host (`hooks.slack.com`) → block-kit; everything else → `{event, message}`. 3-attempt count-based retry. |
| `longbox-scanner`         | ~1000       | Full-walk scan, rescan-unmatched, rematch-for-series. Owns `discovered_folders` upsert, auto-tidy tick, last_matched_count refresh. |
| `longbox-postprocess`     | ~1700       | Phase B. notify-driven watch folder; per-file pipeline (stability → match → ComicInfo rewrite → atomic move → catalog upsert). Owns the in-memory pending-intervention cache. |
| `longbox-pull`            | ~750        | Pull engine. Daily scheduler + manual trigger handle. Poll-in-flight + submit-new phases. Dispatches webhook events. |
| `longbox-scan-scheduler`  | ~200        | Interval-based scan timer (8 scans/24h, every 3h). Pure tokio sleep loop. |
| `longbox-webhooks`        | (above)     |                                                                          |
| `longbox-web`             | (binary)    | Axum HTTP server, route assembly, app state, embedded frontend, startup wiring of all the above. Single binary `longbox`. |

### Dependency graph (architectural rules)

```
longbox-web ──┬── longbox-pull ──┬── longbox-newznab
              │                  ├── longbox-downloader
              │                  ├── longbox-webhooks
              │                  ├── longbox-db
              │                  └── longbox-core
              ├── longbox-postprocess ──┬── longbox-archive
              │                          ├── longbox-db
              │                          └── longbox-core
              ├── longbox-scanner ──┬── longbox-archive
              │                     ├── longbox-db
              │                     └── longbox-core
              ├── longbox-comicvine ── longbox-core (transitively via types)
              ├── longbox-scan-scheduler   (no LongBox deps — pure timer)
              └── longbox-db ── longbox-core
```

**Hard rules (enforced by `Cargo.toml`, not lints):**

- `longbox-pull` **does not** depend on `longbox-comicvine`. The pull sweep operates on issues already in the catalog. Discovering newly-solicited issues is the release calendar's job.
- `longbox-postprocess` **does not** depend on `longbox-comicvine`. Phase B is offline against CV — all metadata comes from the catalog.
- `longbox-postprocess` **does not** depend on `longbox-scanner`. Phase B is event-driven, not walk-driven; the matcher lives in `longbox-core`.
- `longbox-scanner` **does not** depend on `longbox-comicvine`. Scanning is walk + match against the catalog; CV enrichment is a separate /api/series/:id/refresh path.
- `longbox-core` depends on **nothing** in the workspace (it's the leaf). No I/O, no SQL, no HTTP, no time-of-day.

**Shared types** live in `longbox-core`. The DB layer's row types (`SeriesRow`, `IssueRow`, etc. in `longbox-db`) are distinct from the domain types in `longbox-core` (`Series`, `Issue`); the scanner and post-process explicitly translate between them.

---

## 3. Pull engine (A.8) deep dive — priority section

The pull-to-catalog chain has **six hops**. At each one, what crosses the boundary and what identity travels is the key thing — that's where bugs hide.

### Hop 1: pull-list → candidate selection (in-process)

**Triggered by:** `engine::sweep()`, called either by the daily scheduler (`PullConfig::daily_time`, default config) or by the manual "Check now" route `POST /api/pull/check` via `PullHandle::request_sweep()`. The handle uses an `AtomicBool` to refuse overlapping sweeps (returns 409 if one is already running).

**Identity:** the pull engine operates on `(pull_list_row.series_id, issue.id)` pairs. The `issue_repo::list_pull_candidates(series_id)` SQL returns un-owned, non-parked, in-catalog issues whose number is ≥ the pull entry's `start_issue` floor (the floor is applied in Rust via `IssueNumber::natural_cmp` because lexical comparison would wrongly drop "10" below "2").

**Boundary out:** for each candidate, a `(series.title, issue.number, exclude_guids)` tuple is built. `exclude_guids` comes from prior failed attempts that recorded a `release_id` — only *grab* failures carry one (submission failures record none, so the same release is retried). No year is passed at the moment — see open question §12.

### Hop 2: candidate → indexer search (HTTPS)

**Called by:** `longbox_newznab::find_release_excluding(indexers, title, issue, year, exclude_guids)`.

**Indexer order:** sorted by `priority` ASC inside the call (defensive — the repo already returns them ordered). The first indexer that returns *any usable* result wins; per-indexer failures accumulate. If every indexer errors, `NewznabError::AllIndexersFailed(Vec<(IndexerId, IndexerError)>)` bubbles. A mix of errors + clean empties is treated as a clean no-match, not a failure.

**Query construction** (`longbox_newznab::query::build_url`):

```
GET {indexer.base_url}/api
    ?t=search
    &apikey={indexer.api_key}
    &q={search_term}
    &cat=7030             # newznab "comics" category
    &maxage={maxage_days} # default 1500
    &limit=100
    &o=xml
```

`search_term` uses a **two-variation strategy**: zero-padded 3-digit issue first (`Wolverine 005`), unpadded second (`Wolverine 5`). Non-numeric issues (`Annual 1`, `½`) pass through identically in both. Year is optionally appended (` (YYYY)`) for volume disambiguation, but the pull engine currently does **not** pass a year — see open question §12.

**Indexer base_url config trap (live 2026-05-26):** Prowlarr serves indexer-specific newznab feeds at `/<id>` paths. Configuring `base_url=http://192.168.1.163:9696` (root) hits Prowlarr's own REST API and returns `{"current":"v1"}` instead of newznab XML. Configure as `http://192.168.1.163:9696/<id>` instead.

### Hop 3: result selection (in-process)

**`select_best`** ranks releases by:
1. Format (cbz < cbr < unknown — lower = better, so cbz wins)
2. `grabs` descending
3. `published` descending (None sorts last)

**Then exclusion:** any release whose `guid` is in `exclude_guids` is dropped *before* selection. An all-excluded indexer falls through to the next indexer (treated like a zero-hit, not a failure).

**No series-title similarity filter** — see "Issue C" in §9. Currently `Odin 1` happily matches "Beware the Eye of Odin 001"; `The Darkness 1` matches "Justice League - Road To Dark Crisis 001".

### Hop 4: selected release → SAB submit (HTTPS)

**Called by:** `downloader.submit(release.nzb_url, name)` where `name = format!("{series.title} {issue.number}")`.

For SAB:

```
GET {base_url}/api
    ?mode=addurl
    &name={nzb_url}        # the NZB URL Prowlarr served
    &nzbname={name}        # "Wolverine 5"
    &cat={downloader_config.category}
    &apikey={api_key}
    &output=json
```

**Returns:** `{ status: true, nzo_ids: ["SABnzbd_nzo_xxxxx"] }`. LongBox extracts `nzo_ids[0]` as the `DownloadHandle`.

**SAB category trap (live 2026-05-26):** LongBox sends `downloader_config.category = "7030"` (because that's the newznab cat code that came up first when wiring it), but SAB's valid categories are `*, movies, comics, books, tv, prowlarr, music`. SAB falls back to the default `*` category silently — visible only in SAB's history view, not in any return error to LongBox. Recommended fix: set `category = "comics"`.

**Identity at this boundary:**

- LongBox → SAB: NZB URL + display name. **No correlation token** is passed. SAB does not know which `pull_attempt` this is.
- SAB → LongBox: `nzo_id` (the SAB job ID). LongBox stores it as `pull_attempts.download_handle`.

### Hop 5: SAB submit response → `pull_attempts` row

`pull_attempt_repo::insert(NewPullAttempt {…})`:

```
series_id        — from the pull-list entry
issue_id         — from the candidate
indexer_id       — the indexer that served the release
release_id       — release.guid (Prowlarr's guid; used for retry-exclusion)
status           — 'submitted'
download_handle  — SAB's nzo_id
retry_count      — cumulative failed attempts for this issue
unknown_polls    — 0
```

If submit fails (HTTP error, SAB returned status=false, SAB returned auth error), insert a `failed` row instead with `release_id = NULL` (so the same release stays eligible next sweep — the *downloader*, not the NZB, was the suspect).

### Hop 6a: status polling (HTTPS, future sweeps)

**Phase 1 of every sweep** polls every `submitted` attempt before doing any new search work. `downloader.status(&DownloadHandle)`:

For SAB:
1. `mode=queue` first — active jobs live there.
2. `mode=history` second — `Completed | Failed | Verifying | Repairing | …`.
3. Absent from both → `DownloadStatus::Unknown`.

**Unknown handling:** `pull_attempts.unknown_polls` is bumped; transitions to `failed` only after `UNKNOWN_POLL_LIMIT = 3` consecutive Unknowns. A Known status resets the counter to 0. This absorbs SAB flake (a job briefly dropping out of both queue and history during mid-history-write).

**On `DownloadStatus::Failed(reason)`:** record `failed` immediately; fire `maybe_fire_pull_failed`.

### Hop 6b: file landing → Phase B attribution

**This is where the pull-engine attribution actually happens.** And it's metadata-based, not file-name-based.

When SAB writes the completed file to its complete folder (per category settings — must point at LongBox's `/watch` mount), the LongBox post-process watcher (`notify::Watcher`) sees a `Create` or `Modify` event, runs it through `skip::should_skip` (drops `.partial`, dotfiles, non-cbz/cbr extensions, the `_unsorted/` subfolder), and queues the path on a bounded `mpsc::channel<PathBuf>` (capacity 4096).

The **consumer task** drains the channel and calls `processor::process_one(path, library_root, library_root_id, db)`:

1. **Stability check** — sleep until mtime is older than `STABILITY_WINDOW = 2s`. Absorbs `.partial → .cbz` rename arriving before writes settle.
2. **Read ComicInfo.xml** from the archive (CBZ via `zip`, CBR via `libunrar`). Optional — most files don't carry it.
3. **Load parser patterns** from the DB.
4. **Build a `title_hint`** — first from ComicInfo, then from filename parse. Empty hint → straight to `_unsorted/`.
5. **Find candidates** — `find_candidates(db, hint, year_hint)` queries the catalog for plausible matching series.
6. **`match_file`** — runs the cascade (CV web URL > ComicInfo > filename > similarity) and returns `MatchResult { issue_id, confidence, method }`.
7. **Classify** — `classify_status(issue_id, confidence, method, PHASE_B_OWNED_THRESHOLD)` where the threshold is the same `DEFAULT_MATCH_THRESHOLD` Phase A uses. Below threshold → `_unsorted/`.
8. **Imported path** — rewrites the archive (raw-copy CBZ entries, decompress CBR + recompress to CBZ) with a regenerated `ComicInfo.xml`, atomic-renames into `LibraryPath::new(series.title, series.start_year, issue.number).full(library_root)`. **All Phase B output is `.cbz`** — even when the source is CBR.
9. **Pull attribution check:**

```rust
let pulled = pull_attempt_repo::has_in_flight_attempt(db, series.id, issue.id).await?;
let match_method = if pulled { MatchMethod::PullList } else { MatchMethod::PhaseB };
```

10. **If pulled** — `pull_attempt_repo::mark_grabbed_for_issue(db, series.id, issue.id)` settles every in-flight attempt for that issue to `grabbed` (multi-row by design: 2+ races settle together).

**Identity binding mechanism:** the pull engine and the post-processor agree on `(series_id, issue_id)`. There is *no token* propagated through SAB, no canonical filename convention, no NZB-naming contract. The post-processor independently re-derives the (series, issue) by parsing the file + matching against the catalog, then *retroactively* looks up whether any `pull_attempt` row exists for that pair.

**Consequences (this is the open question §12):**
- If Phase B parses the file *wrong*, the pull attempt never gets attributed and the file lands as a generic Phase B catch (`match_method = 'phase_b'`).
- If Phase B parses the file *right* but to a different series than the pull engine intended (false-positive indexer match, e.g. `Odin 1` → "Beware the Eye of Odin 001"), the file lands as `'phase_b'` against the *wrong* series, and the in-flight pull attempt for the *intended* series sits in `submitted` until it times out via the Unknown-poll path — because SAB long since reported it `Completed`.
- The pull_list `MatchMethod::PullList` is therefore a *best-effort attribution*, not a hard guarantee.

### Slack webhook integration

**When events fire** (`longbox-pull::engine`):

- **`pull_failed`** — fires when `pull_attempts` for a given `(series_id, issue_id)` reach `RETRY_CAP = 3` failed attempts. The issue is now parked. Payload: `{ event: "pull_failed", message: "Pull failed permanently: {title} — {reason}" }`. Only on the cap-cross, not on every failure.
- **`pull_engine_error`** — fires when the sweep loop itself errors (DB unreachable, mid-sweep crash). Payload: `{ event: "pull_engine_error", message: "Pull engine error: {error}" }`. Engine-wide, not per-issue.
- **`pull_succeeded`** — *defined but not yet emitted.* Deferred from A.8 Step 10. Future emit point is in `longbox-postprocess::processor::import_as_owned` after `mark_grabbed_for_issue`.
- **`new_solicitations`** — *defined but not yet emitted.* Deferred from A.8 Step 10. Needs a CV-polling delta detector.

**Bitmask** (`webhook_config_repo::EVENT_*`):

```
EVENT_PULL_SUCCEEDED       = 1 << 0
EVENT_PULL_FAILED          = 1 << 1
EVENT_NEW_SOLICITATIONS    = 1 << 2
EVENT_PULL_ENGINE_ERROR    = 1 << 3
EVENT_MASK_ALL             = 15
```

**Delivery** (`longbox-webhooks::deliver`):
- Slack host (`hooks.slack.com`) → block-kit (`{ blocks: [{ type: "section", text: { type: "mrkdwn", text: message }}]}`).
- Anything else → plain `{ event, message }` JSON.
- 3 attempts, 500ms fixed backoff, 10s per-request timeout. Count-based, no persistent retry queue. A target that stays down misses the event permanently.

**Dispatch** (`longbox-pull::dispatch::dispatch`) is fire-and-forget: spawned, never awaited by the sweep. A slow webhook target can't stall the sweep.

### Error paths summary

| Failure                              | Where caught                  | What happens                                                    | Surfaces on                |
| ------------------------------------ | ----------------------------- | --------------------------------------------------------------- | -------------------------- |
| Indexer returns nothing              | `engine::sweep_series`        | No `pull_attempt` row; retried next sweep                       | `summary.no_match`         |
| Every indexer errors                 | `engine::sweep_series`        | No `pull_attempt` row (don't park on infra failure)             | `summary.indexer_errors`   |
| SAB submit fails                     | `engine::sweep_series`        | `pull_attempts` row inserted with `status='failed'`, no `release_id` | `summary.submission_failed`, `/needs-attention` |
| SAB completes wrong file             | (not detected)                | Phase B parses, lands as `phase_b` (or `_unsorted/`), pull attempt times out on Unknown polls | (silent until poll timeout) |
| SAB reports `Failed`                 | `engine::poll_in_flight`      | `pull_attempts.status='failed'` with reason; `maybe_fire_pull_failed` | `summary.grab_failed`, `/needs-attention`, possibly Slack |
| Watch folder file doesn't parse      | `processor::process_one`      | Moves file to `_unsorted/`, inserts as unmatched (`status='unmatched'`) | `/needs-attention` (unmatched), in-memory pending-intervention cache |
| Watch folder file parses to wrong series | `processor::process_one`  | Lands as owned/needs-review against the wrong series; pull attempt sits in `submitted` | (false-positive attachment, silent) |
| Move fails (EXDEV, ENOSPC)           | `processor::commit_move`      | `Outcome::Failed { reason: MoveFailed }`, source stays put      | pending-intervention cache |
| ComicInfo rewrite fails              | `processor::rewrite_to_temp`  | `Outcome::Failed { reason: ComicInfoWriteFailed }`              | pending-intervention cache |
| Target path collision                | `processor::process_one`      | `Outcome::Conflict`, source stays put                           | pending-intervention cache |
| Pull engine sweep itself crashes     | `schedule::scheduler_loop`    | Logs `pull.sweep_failed`; fires `pull_engine_error` webhook     | Slack                      |

---

## 4. Library tidy / reconciliation (A.9)

### `discovered_folders` lifecycle

Top-level subfolders of the library root that look series-shaped but don't resolve to any tracked series. Surfaced in `/library/tidy` for the user to **add** (creates a CV-linked series), **convert** (creates a shallow series and attaches files by filename parsing alone), or **dismiss** (hides permanently).

**Schema (current, post-F6):**

```
discovered_folders(
  id, folder_name UNIQUE, first_seen_at, last_seen_at,
  dismissed_at,      -- user-permanent dismiss (only via /api/reconcile/dismiss)
  auto_dismissed_at, -- state-derived dismiss (post-add, post-convert, F6 scan-end)
  file_count
)
```

**State transitions:**

| Event                                | Action                                         | Column written              |
| ------------------------------------ | ---------------------------------------------- | --------------------------- |
| Scanner detects untracked folder     | `upsert` (creates row OR refreshes file_count, clears auto_dismissed_at) | `last_seen_at`, `auto_dismissed_at = NULL` |
| User clicks Dismiss in /library/tidy | `discovered_folders_repo::dismiss`             | `dismissed_at = now()`      |
| Post-add success                     | `auto_dismiss(folder_name)`                    | `auto_dismissed_at = now()` |
| Post-convert success                 | `auto_dismiss(folder_name)`                    | `auto_dismissed_at = now()` |
| F6 scan-end: folder no longer untracked | `auto_dismiss_not_in(currently_untracked)`  | `auto_dismissed_at = now()` |

**The F6 trap (fixed 2026-05-24):** the original column conflated user and auto dismisses. Once auto-dismissed, a folder could never resurface even when its files became unmatched again, stranding ~26 folders / ~763 files. The split lets auto-dismissed folders re-appear when their files re-qualify.

**`list` filter:** `dismissed_at IS NULL AND auto_dismissed_at IS NULL`. The upsert has `WHERE dismissed_at IS NULL` so user-dismissed rows stay hidden, and the upsert *clears* `auto_dismissed_at` so auto-dismissed rows resurface.

### Scanner pipeline

**Trigger sources:**
- **Scheduled** — `longbox-scan-scheduler` ticks every 3 hours (8 scans/24h, interval-based to dodge timezone entirely). Calls `Scanner::scan_full(library_root_id)`.
- **Manual** — `POST /api/scan/full`, `POST /api/scan/rescan-unmatched`, `POST /api/scan/rematch-for-series/:id`. Same Scanner instance, different entry methods.
- **No watch-folder trigger today** — Phase B's notify watcher only feeds the post-process pipeline, not the scanner. (A.9 future work would add scanner triggering from file events on the library root, but that's not built.)

**`Scanner::scan_full` body:**

1. **Take scan_lock** — `try_lock` to refuse overlapping scans (`ScanError::AlreadyRunning`).
2. **Insert `scan_runs` row** with `kind='full'`, `status='running'`.
3. **Walk the library** via `walker::walk_library(root_path)` (walkdir, follows-links=false).
4. **Per-file:** `process_file` → read ComicInfo → parse filename → find_candidates → match_file → classify_status → file_repo upsert.
5. **Mark missing** — `file_repo::mark_files_not_seen_since(library_root_id, started_at)`: any file row whose `last_seen_at` is older than the scan start has `is_present` flipped to false.
6. **Refresh `series.last_matched_count`** — for transition-phantom detection.
7. **Auto-tidy tick** — `tick_empty_scan_counters` increments `consecutive_empty_scans` for every series with zero present owned files, resets it to 0 for any series with at least one. Threshold = 3 (debounces a transient mount blip).
8. **Auto-tidy mark** — if `settings.auto_tidy_enabled = true`: series past the threshold get `auto_tidy_due_at = now + 14 days`.
9. **Auto-tidy purge** — series whose `auto_tidy_due_at` is in the past get hard-deleted.
10. **`detect_discovered_folders`** — group present files by top-level folder, upsert any folder whose files all failed to resolve into `discovered_folders`, auto-dismiss any open row whose folder is no longer in the untracked set.

### Bulk operations

**`POST /api/reconcile/convert`** — bulk-convert untracked folders to shallow series.

- Frontend: `/library/tidy` page, checkbox-per-row + "Convert N selected" button. Two modes: **link-mode** (attach to existing series matched by `find_for_dedup` if found), **shallow-mode** (always create new).
- Backend: `convert_one_folder` per row. **Bug 1a rollback** (shipped 2026-05-26): if zero files in the folder have parseable issue numbers, the series row is *not* created — returns `ApiError::Unprocessable` instead of creating a ghost.
- **Dedup integration:** `series_repo::find_for_dedup(db, sort_title, start_year)` is called *before* insert. See §6 below for the two-phase logic.

**`POST /api/reconcile/phantoms/bulk`** — bulk-delete transition phantoms (series with `last_matched_count > 0` and current `owned = 0`).

### Auto-tidy

Triggers: scan-end (every full scan). Conditions: `consecutive_empty_scans ≥ 3` AND `auto_tidy_enabled = true` AND not awaiting first download (pull-list series with no successful pull yet get the "phantom" treatment in the UI but auto-tidy ignores them). Soft-delete behavior: a marked series is *not* hidden anywhere — `auto_tidy_due_at` is visible as a countdown in `/library/tidy`. Only the scan-end purge actually deletes (hard delete via CASCADE).

### Dedup logic (`series_repo::find_for_dedup`)

Two-phase CTE. See `longbox-db/src/series_repo.rs:60+` for the full docstring. Summary:

- **Phase 1 (strict, NULL-safe):** match on `(sort_title, start_year)` using SQLite's `IS` so two NULL years dedup against each other but two distinct years don't.
- **Phase 2 (NULL-incoming fallback, Bug 2):** when phase 1 returns nothing AND the incoming `start_year` is NULL, match against any *year-set* row sharing the sort_title. Survivors: cv_id-set first, then year-set, then earliest `created_at`.
- **Asymmetric on purpose:** a year-set incoming does NOT fall back to a NULL-year row (risks linking a reboot to a stale shallow row).
- **Multi-match guard:** if phase 2 finds >1 year-set row sharing the sort_title, refuse the link (1964 + 2024 Daredevil — can't pick confidently). Returns None; the caller creates a new row.

**Cleanup migrations:** two have shipped — `20260523000000_dedup_series.sql` (initial (sort_title, start_year) dedup) and `20260526100000_dedup_series_across_null_year.sql` (Bug 2: phase-2 retroactive merge with `year_set_count <= 1` safety guard). The latter is the one with the auto-run-on-startup miss in §9.

---

## 5. Parser

**Where it lives:** `longbox-core::filename::parse(filename, &patterns)` is the pure function. Patterns come from the DB at scan time (`parsing_pattern_repo::list_enabled`) AND from `default_patterns()` for in-process tests. **Both must stay in lockstep** when adding patterns — the DB seed and the constant are mirrored by hand.

**Cascade order:** patterns are sorted by `priority` ASC at every call; first successful capture wins. A pattern that's `enabled = false` is skipped; a pattern whose regex fails to compile is skipped silently (caller has better logging context).

**Required captures:** `series`, `number`. Optional: `volume`, `year`, `title`. `series` is trimmed.

### The 10 current patterns (post-Bug 1b)

| id | name                                  | priority | shape                                                              | example                                              |
| -- | ------------------------------------- | -------- | ------------------------------------------------------------------ | ---------------------------------------------------- |
| 1  | Series Vol N #M                       | 5        | volume-aware, specific marker first                                | `Saga Vol 1 #1.cbz`                                  |
| 8  | Series vN - Subtitle (YYYY)           | **6**    | TPB volume + subtitle — Bug 1b                                     | `Fear Agent v01 - Re-Ignition (2007).cbz`            |
| 9  | Series Book N (YYYY)                  | **7**    | literal "Book N" — Bug 1b                                          | `Promethea Book 1 (2000).cbz`                        |
| 2  | Series #NNN (YYYY)                    | 10       | strict year-after-number                                           | `Wolverine #5 (2024).cbz`                            |
| 5  | Series N (Xf Y) (YYYY)                | 11       | "X of N" part marker — A.9 parser hot-fix                          | `Saga 5 (1 of 6) (2024).cbz`                         |
| 6  | Series N - Subtitle (YYYY)            | 12       | strict year-after-number-with-subtitle — A.9 parser hot-fix        | `Wolverine 5 - Origins (2024).cbz`                   |
| 7  | Series (YYYY) NNN                     | 15       | **75% of catalog** — year-before-number — A.9 parser hot-fix       | `Wolverine (2024) 5.cbz`                             |
| 3  | Series NNN (YYYY)                     | 20       | strict numeric                                                     | `Wolverine 5 (2024).cbz`                             |
| 10 | Series NNN (YYYY[-MM]) permissive     | **25**   | year-month stamp + permissive tail — Bug 1b                        | `Title 001 (2024-01) - Side Story (digital).cbz`     |
| 4  | Series_NNN or Series NNN              | 30       | **catch-all** — the poison source                                  | `Wolverine_005.cbz` or `Wolverine 005.cbz`           |

### Priority placement principle (Bug 1b lesson)

**Specific markers get low priority** (claim early, before generic patterns absorb them) — pattern 8 (`vN -`) and pattern 9 (`Book N`) sit at 6/7 so a TPB series doesn't get claimed by id=2 as if its volume number were an issue number.

**Permissive shapes get high priority** (sit just above the catch-all) — pattern 10 at 25 only fires after every strict-year pattern (10, 11, 12, 15, 20) has had a shot. This preserves the title capture for filenames like `Saga 001 (2014) - Volume One.cbz` where id=2's greedy `.+` correctly grabs the title; if pattern 10's non-greedy `[^()]+` claimed it first, it'd lose information.

The cascade Bug 1b traced (mid-implementation correction) was the load-bearing exercise: priorities 8/13/14 were the original proposal; pattern-10's `[^()]+` would have stolen Saga's `Volume One` capture, and pattern-8 would have claimed `Promethea Book 1` before pattern-9 did. Shifted to 6/7/25 after the trace.

### Synthesized issue creation

When does it happen: bulk-convert link-mode. The `convert_one_folder` path parses each file's basename through the active pattern set; if a file gets `(series, number)`, the converter calls `issue_repo::upsert_number_only_returning(series_id, number)` — `ON CONFLICT(series_id, number) DO UPDATE SET number = excluded.number RETURNING id, number`. The no-op `DO UPDATE` is the trick to make RETURNING fire on conflict.

The resulting `issues` row has `cv_issue_id IS NULL` — this is **the semantic that flags it as synthesized**. Step 6c (CV enrichment) is supposed to merge against these rather than duplicate.

### Catch-all behavior (pattern id=4, priority 30)

The `Series_NNN or Series NNN` regex matches almost anything ending in `_N.cbz` or ` N.cbz`. When it claims `Wolverine (2024) 005.cbz` (before patterns 7/15 existed), it baked the `(2024)` into `series_title = "Wolverine (2024)"`, which then poisoned the title-similarity match against CV's clean `sort_title = "wolverine"` — dropping the file to unmatched/needs_review. Bulk-convert was unaffected because it only uses `parsed.number` and discards `series_title` (same parser, different consumer).

**This is the match-but-poison surface** (deferred item, §9):
- A file lands in the catalog with `match_method='filename_regex'` and a corrupted title.
- The scanner has no way to tell "parsed cleanly" from "parsed but the title is structurally wrong."
- Additional data point: `I Hate Fairyland (2022) (2022)` (duplicated `(YYYY)` in the folder name) — pattern 7/15 absorbs it but bakes one year into series_title.

---

## 6. Schema

### Tables (post all migrations through 2026-05-26)

**Identity columns and FKs that matter for dedup / pull-attribution / lookup are bolded.**

#### `library_roots`

```
id PK, path UNIQUE, created_at
```

#### `series`

```
id PK, cv_id UNIQUE NULLABLE, metron_id UNIQUE NULLABLE,
title, sort_title, start_year NULLABLE, publisher NULLABLE,
description, cover_url, created_at, updated_at,
last_matched_count (A.9), consecutive_empty_scans (A.9), auto_tidy_due_at (A.9)
```

Index: `idx_series_sort_title`.

- **Shallow series** = `cv_id IS NULL` (bulk-converted, no CV link). First-class citizens since 6a — every CV-keyed affordance must gate on `cv_id`.
- **`(sort_title, start_year)` is the dedup key.** Not a UNIQUE constraint at the DB level — enforced in `find_for_dedup` because the year is nullable and the dedup logic is two-phase.

#### `issues`

```
id PK,
series_id FK→series CASCADE, cv_issue_id UNIQUE NULLABLE, metron_issue_id UNIQUE,
number, title, cover_date, summary, cover_url, created_at, updated_at,
UNIQUE(series_id, number)
```

Index: `idx_issues_series`.

- **Synthesized issue** = `cv_issue_id IS NULL`. Bulk-convert creates these.
- **`(series_id, number)` UNIQUE** — this is the constraint pattern id=8 (vN-as-issue) would collide against if a hybrid catalog had both `#1` and `v01`.

#### `files`

```
id PK,
issue_id FK→issues ON DELETE SET NULL,  -- soft-detach on issue delete
library_root_id FK→library_roots,
path_relative, size_bytes, mtime, last_scanned_at,
match_method TEXT, match_confidence REAL, status TEXT,
cached_comicinfo_xml, cached_at,
is_present (Step 6516), last_seen_at, matched_at (Phase A.5/A.6),
UNIQUE(library_root_id, path_relative)
```

Indexes: `idx_files_issue`, `idx_files_status`, `idx_files_match_method`.

- **`match_method`** open enum (TEXT, no CHECK): `web_url_cv`, `web_url_metron`, `comicinfo_xml`, `filename_regex`, `manual`, `phase_b`, `pull_list`, `unmatched`, `ignored`.
- **`status`** open enum: `owned`, `needs_review`, `unmatched`, `ignored`.
- **`match_method='pull_list'`** is the durable record of pull-engine attribution.

#### `scan_runs`

```
id PK, library_root_id FK, started_at, finished_at,
files_seen/added/updated/matched/needs_review/unmatched (counters),
status TEXT DEFAULT 'running', error_message, kind (A.5: 'full' | 'rescan_unmatched' | 'rematch_for_series')
```

#### `parsing_patterns`

```
id PK AUTOINCREMENT, name, pattern, priority, enabled, created_at
```

10 seed rows (current), priorities `[5, 6, 7, 10, 11, 12, 15, 20, 25, 30]`.

#### `settings`

Key-value bag. Keys today include `match_confidence_threshold`, `auto_tidy_enabled`, scan/pull interval keys.

#### `discovered_folders`

(See §4 for lifecycle.)

```
id PK, folder_name UNIQUE, first_seen_at, last_seen_at,
dismissed_at, auto_dismissed_at, file_count
```

#### `pull_list`

```
id PK, series_id FK UNIQUE CASCADE, added_at, start_issue NULLABLE,
paused INT, last_pull_attempt_at, last_successful_pull_at, failure_count
```

- One row per series. `start_issue` is TEXT (issues are TEXT: "Annual 1", "½").

#### `pull_attempts`

```
id PK,
series_id FK CASCADE, issue_id FK CASCADE, indexer_id FK SET NULL,
attempted_at, release_id NULLABLE, status CHECK ('pending'|'submitted'|'grabbed'|'failed'|'mismatched'),
error_message, retry_count, download_handle NULLABLE (A.8 Step 6),
unknown_polls (A.8 Step 6)
```

Index: `idx_pull_attempts_series_issue` — used by Phase B's `has_in_flight_attempt(series_id, issue_id)`.

#### `downloader_config`

```
id PK CHECK(id=1) -- single-row, INSERT OR REPLACE on id=1
kind CHECK('sab'|'nzbget'), base_url, username (nzbget only), secret, category, enabled, updated_at
```

#### `indexer_configs`

```
id PK AUTOINCREMENT, name UNIQUE, base_url, api_key, enabled,
priority (asc = first), maxage_days DEFAULT 1500, created_at
```

#### `webhook_configs`

```
id PK, name UNIQUE, url, event_mask INT (EVENT_* bitset), enabled, created_at
```

#### `cv_release_cache`

```
id PK, date_from, date_to, publisher, payload_json, cached_at,
UNIQUE(date_from, date_to, publisher)
```

CV release calendar projection, JSON blob, TTL is read-side policy.

### Key relationships

```
series 1───* issues 1───* files
       └───? pull_list (0 or 1)
       └───* pull_attempts (via series_id and issue_id)
       └───* discovered_folders (loose match via series.title ≈ folder_name)

library_roots 1───* files
              └──* scan_runs
```

### Soft-delete vs hard-delete

| Operation                        | Behavior                                             |
| -------------------------------- | ---------------------------------------------------- |
| Delete series                    | Hard delete; CASCADE drops issues, pull_list, pull_attempts; files.issue_id → NULL via SET NULL (file rows survive) |
| Delete issue                     | Hard delete; files.issue_id → NULL                   |
| File missing from disk           | Soft (`is_present=0`); row survives                  |
| Auto-tidy mark                   | Soft (`auto_tidy_due_at` set); UI countdown          |
| Auto-tidy purge (after recovery) | Hard delete                                          |
| Discovered folder dismiss        | Soft (`dismissed_at`/`auto_dismissed_at`)            |

### Migration state

14 migrations total. List (oldest first):

```
20260516040415_initial.sql                            ← initial schema + 4 parser patterns
20260516062118_add_file_presence_tracking.sql         ← is_present, last_seen_at
20260517000000_add_scan_run_kind.sql                  ← scan_runs.kind
20260518000000_add_files_matched_at.sql               ← files.matched_at
20260519000000_add_publisher_filters.sql              ← publisher_filters table
20260519100000_add_pull_list_tables.sql               ← pull_list, pull_attempts, indexer_configs,
                                                         downloader_config, webhook_configs, cv_release_cache
20260520000000_add_pull_attempt_tracking.sql          ← pull_attempts.download_handle + unknown_polls
20260520100000_add_library_tidy_tables.sql            ← series.last_matched_count, discovered_folders
20260522000000_add_auto_tidy.sql                      ← series.consecutive_empty_scans, auto_tidy_due_at
20260523000000_dedup_series.sql                       ← initial dedup cleanup
20260524000000_add_year_first_parser_patterns.sql     ← +3 parser patterns (id 5/6/7 at 11/12/15)
20260524100000_split_dismiss_source.sql               ← F6 split: dismissed_at + auto_dismissed_at
20260526000000_add_permissive_parser_patterns.sql     ← Bug 1b: +3 parser patterns (id 8/9/10 at 6/7/25)
20260526100000_dedup_series_across_null_year.sql      ← Bug 2: phase-2 retroactive merge
```

**Auto-apply mechanism:** `sqlx::migrate!()` macro embeds the migrations into the binary and runs them on startup via `migrate!().run(&pool)`. Confirmed embedded (`strings`) for the last migration.

**Migration-not-applied-on-startup caveat:** the final migration (`20260526100000`) did NOT auto-run on container startup despite `--no-cache` rebuild; was applied via `sqlx migrate run` CLI + DB swap. Reproduction scope unknown — possibly content-shape, possibly a `migrate!` macro quirk with the CTE form, possibly a Docker context artifact. Tracked as deferred item §9; if it recurs, becomes a workflow rule.

---

## 7. External integrations

### ComicVine

- **Endpoint root:** `https://comicvine.gamespot.com/api/`
- **Auth:** `api_key` query param (from `COMICVINE_API_KEY` env).
- **Rate limit:** 180 req/h, burst 5, max wait for slot — token bucket via `governor`.
- **Endpoints actually called:** `search_volumes` (CV-add UI), `fetch_volume(cv_volume_id)`, `fetch_issues(cv_volume_id)`, `fetch_release_calendar(date_from, date_to, publisher)`.
- **Caching:** the release calendar lives in `cv_release_cache` (read-side TTL: short for the calendar view, daily for pull-engine reads). Volume/issue fetches are not cached at this layer — they refresh on demand via `POST /api/series/:id/refresh`.
- **Refresh trigger:** manual only today. No background polling of CV.

### SABnzbd

- **Endpoint:** `{base_url}/api`. Single endpoint, mode-discriminated.
- **Auth:** `apikey` query param. Bad key returns plain-text `error: API Key Incorrect` (not JSON) — `longbox-downloader::sabnzbd::get` detects the prefix and maps to `AuthFailed`.
- **Modes used:** `addurl` (submit), `queue` (active jobs), `history` (completed/failed), `version` (not used — `queue?limit=0` is the auth probe instead, because `version` is unauthenticated on most builds).
- **Submit payload:** `?mode=addurl&name={nzb_url}&nzbname={display_name}&cat={category}&apikey={key}&output=json`.
- **Category config:** `downloader_config.category`. Currently set to `7030` (newznab cat code) but SAB needs one of its own configured names (`*`, `comics`, etc.). See §3 Hop 4 trap.
- **Post-process script integration:** none. LongBox polls SAB via `mode=queue|history` rather than receiving callbacks. SAB writes the completed file to its category complete_dir; LongBox sees it via notify on the watch folder. **There is no LongBox SAB post-process script.**

### Prowlarr

- **Connected:** yes (as the configured newznab indexer).
- **Role:** meta-indexer / aggregator. Provides a per-indexer-id newznab proxy at `{prowlarr_host}/<id>` paths. LongBox sees it as a single Newznab endpoint.
- **base_url config:** must include the per-indexer path (`/1`, `/2`, …). The Prowlarr API root returns `{"current":"v1"}` instead of newznab XML.

### Slack

- **Webhook URL config:** stored in `webhook_configs.url`. Detected by host (`hooks.slack.com`) for block-kit formatting.
- **event_mask bitset:** `EVENT_PULL_SUCCEEDED|EVENT_PULL_FAILED|EVENT_NEW_SOLICITATIONS|EVENT_PULL_ENGINE_ERROR` = 1, 2, 4, 8. `EVENT_MASK_ALL = 15`. Current webhook is configured with `event_mask = 15`.
- **Events emitted today:** `pull_failed`, `pull_engine_error`. The other two are deferred (§9).

---

## 8. Workflow rules

The nine rules currently codified in the A.9 prompt:

1. **`--force-recreate` on redeploy** — `docker-compose up -d` alone no-ops on stale images; `--force-recreate` always rebuilds the container against the freshly-built image.
2. **`cargo clippy --workspace --all-targets`** — not just default scope; test code surfaces lints the default scope misses.
3. **`sqlx prepare --workspace -- --all-targets`** — same reason; the `.sqlx` offline cache must cover test queries or the SQLX_OFFLINE container build fails.
4. **Dockerfile crate-COPY rule** — every new workspace crate needs its stage-2 `COPY` line added in the same commit.
5. **Colima mount fragility** — any unmount/remount of the host comics volume requires `colima restart` before trusting a scan.
6. **Idempotency on new insertion paths** — when a repo has idempotency on one key shape (e.g. `cv_id`) and a new write path uses a different shape, evaluate independently. The 6a bulk-convert hot-fix surfaced this with `(sort_title, start_year)`.
7. **Spot-check anomalies as samples** — a small visible anomaly is usually a sample of a hidden population, not an isolated outlier. The 6a→6b cleanup→parser sequence: 2 visible misses were actually a 4,182-file gap.
8. **Cross-check catalog state against disk state** — single-source data has blind spots that surface as quantitative-criterion misses post-deploy. Walk the disk per-folder when verifying parsing/attachment fixes.
9. **Every CV-keyed affordance gates on `cv_id`** — Refresh buttons, ComicVine deep-link buttons, CV-only hints. Shallow series are first-class.

**Uncodified patterns followed in practice:**

- **Per-step kickoff → user approval → single implementation commit → container rebuild → verify** is the actual work cadence, not just A.9's. Pre-A.9 the same pattern was used; A.9 just made it explicit.
- **Single-concern commits with bisection value** — Bug 1 was split into 1a (structural rollback) + 1b (parser patterns) to preserve bisectable history.
- **Defer fixes that span design surfaces** — match-but-poison signal is the canonical example. Hot-fix would be wrong; needs its own design conversation.
- **Tracing the cascade before placing priorities** — Bug 1b's mid-implementation correction shipped because the trace happened first. Codify if it recurs.

---

## 9. Deferred items — full catalog

Every deferred issue currently in flight. Sources: `longbox-phase-a9-prompt.md` Deferred section + the in-flight 2026-05-26 conversation.

### From `longbox-phase-a9-prompt.md`

| # | Item                                       | Where it surfaced                          | Why deferred                                         | Unblock condition                                          |
| - | ------------------------------------------ | ------------------------------------------ | ---------------------------------------------------- | ---------------------------------------------------------- |
| 1 | CBR/CBZ duplicate conflict                 | F5 hot-fix (`f57a21c`)                     | Phase B's conflict check tests only the cbz target; a pre-existing `.cbr` and a Phase B `.cbz` re-download coexist | Widen conflict check to also test the cbr sibling           |
| 2 | `needs_review`-as-resolved edge            | F6 dismiss-trap archaeology                | `detect_discovered_folders` treats any `issue_id`-bearing file as "resolved", so weak matches mask real untracked folders | Change predicate to `status='owned'` only                  |
| 3 | `pull_succeeded` webhook event             | A.8 Step 10                                | Needs Phase B's submitted→grabbed transition (it has it now — could ship) | Emit from `import_as_owned` after `mark_grabbed_for_issue` |
| 4 | `new_solicitations` webhook event          | A.8 Step 10                                | Needs CV-polling delta detector — no background CV poller exists | Build the poller; not on the A.9 roadmap                   |
| 5 | **Catch-all parser match-but-poison signal** (LOAD-BEARING) | Parser hot-fix → F6 archaeology | Two competing design moves (`low_confidence: bool` vs catch-all refuse-to-match); choice affects every future catch-all-style pattern | Dedicated design surface; needs Jude / Jeremy thought |
| 6 | vN-as-issue-number collision (Bug 1b id=8) | Bug 1b kickoff                             | Pattern id=8 maps volume `N` to issue number — pragmatic for TPB-only series, but a hybrid catalog with `#1` *and* `v01` would collide on the `(series_id, number)` unique constraint | Either split shallow series by volume, or refuse-to-match when both shapes exist for one series |
| 7 | Bug 2 migration didn't auto-apply on startup | 2026-05-26 deploy                       | `20260526100000_dedup_series_across_null_year.sql` is embedded but didn't run on startup. Root cause unknown | Either reproduce + diagnose, OR establish "CLI apply belt-and-suspenders" workflow rule |

### Surfaced 2026-05-26 (Scenario 1 smoke)

| # | Item                                       | Where it surfaced                          | Status                                               |
| - | ------------------------------------------ | ------------------------------------------ | ---------------------------------------------------- |
| 8 | **Issue A** — no filesystem bridge between Windows SAB and Mac watch folder | Scenario 1 watch-folder check  | Config-only; needs local SAB install OR SMB/NFS share. Not a code bug. |
| 9 | **Issue B** — SAB ignores `downloader_config.category = "7030"`, uses `*` instead | Scenario 1 SAB history check   | Trivial config fix (`category = "comics"`); flagged for record |
| 10 | **Issue C** — newznab `select_best` doesn't filter by series-title similarity, and pull engine doesn't pass `year` to `find_release_excluding` | Scenario 1 SAB history reveal: false-positive matches (Odin 1 → Beware the Eye of Odin 001, The Darkness 1 → Justice League - Road To Dark Crisis 001) | **Real code bug** — codified in `longbox-phase-a9-prompt.md` deferred section 2026-05-28. Two stacking moves: post-filter pool by title similarity + pass `series.start_year` from the catalog. Independent of the smoke blocker |

### Match-but-poison surface — motivating data

The deferred item §5 is the most architecturally load-bearing. Data points accumulated:

- **Parser pattern 4 catch-all corruption** — `Wolverine (2024) 005.cbz` claimed by catch-all with `series_title = "Wolverine (2024)"`. 4,182 files affected at one point.
- **Duplicated-year shape** — `I Hate Fairyland (2022) (2022)` absorbed by pattern 7/15 with one year baked into series_title.
- **Bug 1a zero-attachment ghosts** — convert created series rows with zero attached files, surfacing as `owned=0, total=0`. Hidden until per-folder cross-check exposed them.
- **Pattern 14 / id=8 vN-as-issue semantic** — a parser match on a TPB volume is pragmatically right for TPB-only series but semantically wrong for hybrid catalogs — and the system has no way to express the distinction.

The common shape: **a downstream consumer (catalog, scanner, dedup) treats "parser returned Some" as "we have signal", when in fact the parser returned a structurally-malformed result**. The two candidate design moves (annotate vs refuse) trade off coverage against silent corruption. This is the conversation worth having with Jude before any next hot-fix touches the parser.

---

## 10. Data state today

**Note:** these are the values established at the end of the 2026-05-26 session, before /exit. Service is not running right now (Colima stopped), so these are *snapshot-as-of-then*, not live-as-of-2026-05-28. Re-run the catalog-math invariant on next deploy to confirm.

### Series

- **Total:** 610 (down from 614 after Bug 2 cleanup merged 4 dupes to 2)
- **CV-linked:** majority. Bulk of the catalog is CV-add or refresh-derived.
- **Shallow (`cv_id IS NULL`):** the bulk-converted residue, ~30-ish (informal — hasn't been counted post Bug 2).

### Issues

- **Total:** not measured this session; not load-bearing here.
- **Synthesized (`cv_issue_id IS NULL`):** the issues created by bulk-convert link-mode + shallow-mode paths. Step 6c's job to enrich these by merging against CV fetches.
- **CV-linked (`cv_issue_id` set):** the rest.

### Files (post Bug 2 verification, catalog-math invariant green)

| Bucket          | Count |
| --------------- | ----- |
| Present total   | 4930  |
| Owned           | 4703  |
| Needs review    | 118   |
| Unmatched       | 109   |
| Untracked (folders) | 0 |

Invariant: `4930 = 4703 + 118 + 0 + 109` ✓

### Discovered folders

- **Untracked:** 0
- **User-dismissed:** non-zero (historical user dismisses; not counted)
- **Auto-dismissed:** non-zero (mostly F6 post-add/post-convert successes; not counted)
- **Linked:** N/A — no linked state in the schema; "linked" means "no longer untracked".

### Dupe groups remaining

- **0** as of Bug 2 verification.

### Pull-list

- **Total entries:** 19 + however many were added during Scenario 1 prep (Wolverine 2024 was added; not sure if others)
- **Active vs completed:** the schema doesn't have a "completed" state — `pull_list` rows are subscriptions, not jobs. Active = `paused = 0`.

---

## 11. Phase landscape

### A.5 — Scanner reliability + match-method tracking

**Scope:** scanner subsystem hardening. Added `match_method` enum to `files`, `matched_at` timestamp, `scan_runs.kind` discriminator, rescan-unmatched + rematch-for-series surgical re-scan paths, mark-missing pass.

**Status:** shipped. Closeout doc owes the paired sign-off with A.8 — A.5 closeout is bundled into the A.8/A.5 closeout pair (task #153, pending smoke green).

### A.8 — Pull engine

**Scope:** end-to-end pull-to-catalog chain. Brief covered 13 numbered steps. Highlights:

- Step 3: pull_list / pull_attempts / indexer_configs / downloader_config / webhook_configs / cv_release_cache schema.
- Step 4: newznab client (two-variation search + cross-indexer select + retry-exclusion).
- Step 5: downloader trait + SAB + NZBGet impls.
- Step 6: post-process pull-attribution + download_handle tracking + unknown-poll handling.
- Step 8: release calendar + pull-list bulk add.
- Step 10: webhook delivery + dispatch.
- Step 11: needs-attention surface + retry UI.
- Step 12: full integration test pass (workspace tests green).
- Step 13: end-to-end smoke validation (**this is what's pending**).

**Current state:** Steps 1–12 all shipped. Step 13 smoke pending. Scenario 1 (pull-to-catalog happy path) blocked on Issues A + B (config) + showed Issue C (newznab false-positives). Scenarios 2 (failure paths) and 3 (webhook delivery) untouched.

**Known gaps (smoke-surfaced):** §9 items 8/9/10. None of A.8's *implementation* gaps — the engine works as designed; the surface is integration-level.

### A.9 — Library tidy accuracy + UX

**Scope:** 11 items, 7 numbered steps (Step 6 has sub-steps 6a/6b/6c).

| Step | Items | Status |
| ---- | ----- | ------ |
| 1 — UI polish sweep | 4, 11 | queued |
| 2 — Calendar solicitation completeness | 8 | queued |
| 3 — Publisher grouping on calendar | 1 | queued |
| 4 — Bulk add-to-pull-list + of-note badge | 6, 7 | queued |
| 5 — Scheduling & timezone | 3, 5 | queued |
| 6a — Bulk-convert dedup | — | **shipped** (`c9f0013`) |
| 6b — Auto-tidy on folder removal | 2, 10 | **shipped** (`97d146f`) |
| 6c — CV enrichment | — | queued (task #154) — needs Step 6c kickoff |
| 7 — Missing-issue resolution | 9 | queued |

**Hot-fixes shipped on top of 6a/6b:**
- `c9f0013` dedup phase 1
- `00c6f9c` parser year-first patterns
- `ac11c3b` F6 dismiss-trap split
- `1db527a` shallow-series UX gate
- `8f84f8f` clippy hygiene
- `0033af0` Bug 1a structural rollback
- `f2ee4d5` Bug 1b three permissive parser patterns
- `30d679c` Bug 2 phase-2 fallback + cleanup migration

### Phase B (continuing) — Post-process watch folder

**Scope:** event-driven import of files landing in `/watch`. Shipped through Step 6 of the original Phase B brief. What's deferred to **Phase B+** queue:

- **CBR/CBZ duplicate conflict** (§9 item 1) — needs a sibling-extension check on conflict.
- **Direct-to-.cbz slow-write edge** — current 2s stability window handles `.partial → .cbz` rename but not a slow-writing direct write to `.cbz`. Phase B+ if it surfaces in practice.
- **Filewatch trigger of scanner** — currently notify only feeds Phase B; the library scanner has no event trigger. Future: library-side notify watcher for scan-on-change.

---

## 12. Open architectural questions for Jude

These are the questions where the system has **made a design choice that hasn't been re-examined** and where Jude's second pair of eyes would be most valuable.

### Q1: Pull engine identity binding — metadata-based vs filename-based vs token-based

**Current design:** the pull engine and the post-process pipeline agree on `(series_id, issue_id)`. No correlation token traverses SAB; no filename convention binds the NZB; no canonical naming on receipt.

**Implications observed in Scenario 1 smoke:**
- False-positive indexer matches (Odin 1 → Beware the Eye of Odin 001) result in the *intended* pull attempt sitting in `submitted` until Unknown-poll timeout, while the *wrong* file lands as a generic Phase B catch under the wrong series.
- A user manually dropping a randomly-named CBZ into the watch folder is indistinguishable from a pull-engine grab — both round-trip through the same parser → match → attribute path.

**Alternatives worth considering:**
1. **Pass a correlation token** (e.g. encode `pull_attempt_id` in the SAB `nzbname` or as a SAB script param). Receive-side reads the token, looks up the attempt directly.
2. **Canonical NZB naming on submit** — rename inside SAB via `nzbname` to a deterministic shape (`pull_{attempt_id}_{series.id}_{issue.id}.nzb`). Phase B parses the shape directly.
3. **Status keyed on `download_handle`** — Phase B looks up the `pull_attempt` whose `download_handle` matches the SAB job ID that produced the file. Requires Phase B to ask SAB which job a path belongs to — SAB exposes path-in-history but it's clunky.
4. **Keep the current design** and add a series-title-similarity filter on `select_best` — solves the false-positive problem without changing the binding mechanism.

The right answer depends on how much we trust newznab `select_best` to do the right thing. Currently: not very. Worth Jude's read.

### Q2: Volume-as-issue semantic collision (Pattern id=8)

**Current design:** pattern id=8 maps a TPB volume number to the issue number — pragmatically right for TPB-only series (Fear Agent, Promethea) where there's no per-issue catalog, but semantically wrong for a hybrid catalog (Saga has both `#1` and `vol 1`).

**Today:** the `(series_id, number)` UNIQUE constraint will fire if a hybrid series exists and someone bulk-converts a TPB shape onto it.

**Options:**
1. **Volume-aware schema** — `issues.volume` column, change UNIQUE to `(series_id, volume, number)`. Big change for an edge case.
2. **Refuse pattern 8 when the series has any single-issue file** — context-sensitive parsing. Hard to do because the parser is pure (`longbox-core`, no DB).
3. **Surface the collision and fail the convert** — let the user disambiguate by splitting the folder. Punts to UX.

This connects to Q1: if we had a richer pull-attribution token, "this file was meant to be the TPB" could be a known fact rather than a parser inference.

### Q3: Migration-not-applied-on-startup root cause

**Observed:** `20260526100000_dedup_series_across_null_year.sql` is embedded in the binary (confirmed via `strings`), runs cleanly via `sqlx migrate run` CLI against the same DB, but did NOT auto-run on container startup despite `--no-cache` rebuild.

**Hypotheses worth checking:**
- **sqlx 0.7 migrate macro quirk with `WITH RECURSIVE` or temp tables** — the migration uses both. Worth a minimal repro.
- **Migration version comparison bug** — the date prefix `20260526100000` is correctly larger than `20260526000000`. Could be timestamp comparison vs string comparison issue.
- **Docker layer caching artifact** — `.sqlx` cache mismatch could silently skip a migration. The `SQLX_OFFLINE=true` env in the Dockerfile doesn't affect runtime migrations, but it's worth checking that the `.sqlx` regen happened for this migration.
- **Filesystem ordering inside the binary** — the migrations are embedded by directory order; if sqlx is iterating in a way that's sensitive to that, a file with an unusual name could be skipped.

**If it recurs on the next migration**, becomes a workflow rule: "apply migrations via `sqlx migrate run` CLI as belt-and-suspenders after every hot-fix deploy". If it doesn't recur, file as one-off and move on.

---

## Appendix: repo paths Jude might want to reference

**Pull engine:**
- `longbox-pull/src/engine.rs` — sweep, poll_in_flight, sweep_series, retry-exclusion
- `longbox-pull/src/schedule.rs` — daily scheduler + PullHandle
- `longbox-pull/src/dispatch.rs` — webhook fan-out
- `longbox-newznab/src/client.rs` — find_release_excluding
- `longbox-newznab/src/select.rs` — release ranking (no title filter — see Issue C)
- `longbox-downloader/src/sabnzbd.rs` — SAB client

**Post-process:**
- `longbox-postprocess/src/lib.rs` — watcher setup, channel plumbing
- `longbox-postprocess/src/processor.rs` — per-file pipeline, pull-attribution

**Scanner / reconcile:**
- `longbox-scanner/src/scanner.rs` — scan_full, auto-tidy tick, discovered_folders detection
- `longbox-web/src/routes/reconcile.rs` — bulk-convert, add, dismiss
- `longbox-db/src/series_repo.rs` — find_for_dedup two-phase logic
- `longbox-db/src/discovered_folders_repo.rs` — F6 split

**Parser:**
- `longbox-core/src/filename.rs` — pure parser + default_patterns()
- `longbox-db/migrations/20260516040415_initial.sql` — initial 4 patterns
- `longbox-db/migrations/20260524000000_add_year_first_parser_patterns.sql` — A.9 +3
- `longbox-db/migrations/20260526000000_add_permissive_parser_patterns.sql` — Bug 1b +3

**Webhooks:**
- `longbox-webhooks/src/lib.rs` — Slack vs generic body
- `longbox-db/src/webhook_config_repo.rs` — EVENT_* constants

**Phase plans + closeouts:**
- `longbox-phase-a-prompt.md` (foundational; very large)
- `longbox-phase-a5-closeout.md`
- `longbox-phase-a8-prompt.md`, `longbox-phase-a8-closeout.md`
- `longbox-phase-a9-prompt.md` — workflow rules + deferred items
- `longbox-phase-b-prompt.md`, `longbox-phase-b-known-limitations.md`, `longbox-phase-b-plus-queue.md`
- `longbox-a9-handoff-to-jude.md` — 2026-05-26 work-in-flight handoff

---

**End of snapshot.** Update sections as the system evolves; data-state (§10) decays fastest.
