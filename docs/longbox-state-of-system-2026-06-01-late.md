# LongBox state-of-system snapshot

Date: 2026-06-01, late.
Audience: future-Claude / Jude picking up the LongBox thread.
Supersedes: `longbox-state-of-system-2026-06-01.md` (still untracked
in this working tree, but stale — was written before the two pull-
engine commits below). Earlier `longbox-state-of-system-2026-05-31-late.md`
is in history at fa35767.

## 1. What closed this session

Five commits forming one coherent arc — **Phase B safety hardening
followed by pull-engine manual control surfaces**. The first three
make sure the existing pipelines don't silently corrupt or miss work;
the last two give the user direct control over what the engine
searches.

```
664c054 a9 pull engine: per-issue Search button for Missing issues on series detail
1d88414 a9 pull engine: on-demand single-series search + auto-trigger on subscribe
b458819 a9 scanner: mount-health preflight guards scan_full + rescan_unmatched
48af6cb a9 Phase B: write MetronInfo.xml alongside ComicInfo.xml on import
d0c4ba1 a9 Phase B: PollWatcher fallback for virtiofs-blind inotify
```

### d0c4ba1 — PollWatcher fallback

`notify::RecommendedWatcher`/`INotifyWatcher` is blind to host writes
through Docker Desktop's virtiofs. Host-side download = invisible for
44+ minutes. Swapped to `notify::PollWatcher` — same trait, same
EventKind shape, closure body unchanged. Interval from
`phase_b_poll_interval_seconds` (default 30s). Migration
`20260601020000`. `initial_sweep` stays — PollWatcher's first poll
silently populates without emitting events. Live probe verified:
T+0 host write → T+25 import.

### 48af6cb — MetronInfo.xml on import

`import_as_owned` writes both `ComicInfo.xml` AND `MetronInfo.xml` at
archive root. MetronInfo's whole point is source-tagged `<IDS>` — CV
ID marked `primary="true"`, Metron ID a sibling when present.
Compatibility with Perdoo, ComicRack CE, Comicbox, Codex,
Metron-Tagger. New `longbox-core/src/metroninfo_writer.rs` mirrors
`comicinfo_writer.rs` (pure-text generator, no I/O). XML helpers
copied per the kickoff — refactor only when a third writer appears.
`compose_metroninfo_metadata` builds the write set from
`(SeriesRow, IssueRow)` — zero new DB queries. Live probe: real
catalog series imported with full MetronInfo carrying CV ID, IDW
publisher, sort name, cover date, full CV summary CDATA-wrapped,
RFC 3339 LastModified.

### b458819 — Scanner mount-health preflight

The catastrophe this prevents: SMB share drops, bind mount goes empty
inside the container, 03:00 UTC scheduled scan fires, walk yields
zero files, `mark_files_not_seen_since` flips every catalogued file
to `is_present=false`, `tick_empty_scan_counters` advances every
series's auto-tidy doomsday counter, after three empty scans series
get marked for removal, after 14 wall-clock days they're
hard-deleted.

New `Scanner::preflight_library_root` does a two-tier check:
`read_dir` errors → mount inaccessible, OR `read_dir` succeeds but
yields zero entries → mount stale. Called BEFORE `scan_run_repo::insert`
so the cascade is fully unreachable on failure — no scan_run row
recorded, mark-missing never runs, auto-tidy clock never ticks. Maps
to a 503 with code `scan_preflight_failed`. Load-bearing test asserts
all four invariants directly (typed Err returned, zero new
`scan_runs` rows, no `files.is_present` flip, no
`series.consecutive_empty_scans` advance).

Pre-existing tests that intentionally used empty library dirs as a
"no comics" stand-in got a one-byte `.placeholder` file added — the
walker filters by extension so non-cbz/cbr files are invisible to
scan logic, but their presence keeps the preflight happy.

### 1d88414 — On-demand single-series pull search + auto-trigger

New endpoint `POST /api/pull/search/:series_id`:
- 202 on accepted
- 404 `not_found.pull_list entry` if not subscribed
- 409 `conflict.pull_search_running` if a search for that same series
  is in flight (per-series guard, **independent of the daily-sweep
  global lock** that gates `/pull/check`)

`longbox-pull::sweep_single_series` shares per-sweep prep
(downloader / indexers / patterns / threshold) with the all-series
sweep via a refactored `load_sweep_context`, then runs the existing
private `sweep_series()` for one entry. Skips Phase 1 — in-flight
polling belongs to the daily scheduler.

`PullSearchHandle` is the new per-series concurrency tracker
(`Arc<Mutex<HashSet<i64>>>`). Two searches for distinct series run in
parallel; two for the same return 409. A daily sweep running
concurrently does NOT block. Documented race surface: a daily sweep
mid-series can collide with an on-demand search on the same series.
Cost is one wasted indexer query — `pull_attempts` has no UNIQUE on
`(series_id, issue_id)` so the duplicate IS a real second attempt;
acceptable as a rare collision.

Auto-trigger wired at both subscribe paths: `routes/pull.rs::add`
success arm + `routes/calendar.rs::try_add_one` "added" transition.
Fire-and-forget; a no-op on the per-series guard's `false` return
absorbs duplicate fires when bulk-add has duplicate series_ids.

Frontend: per-row "Search now" button on the pull-list page with a
15s debounce (chosen to cover typical indexer wall time).

### 664c054 — Per-issue Search button for Missing issues

New endpoint `POST /api/pull/search/:series_id/issue/:issue_id`:
- 202 on accepted
- 404 `not_found.series` or `not_found.issue` (the latter also covers
  issue-belongs-to-different-series URL tampering)
- No 409 — in-flight guard lives in the engine and silently skips

**The series does NOT need to be on the pull list** — this is the
"found a gap, fill it" path off series detail.

`sweep_single_issue` bypasses two of the standard sweep gates:
list_pull_candidates's start_floor AND the `retry_count >= 3`
parking (manual override). **Crucial discovery from archaeology**:
`pull_attempts` has NO unique constraint on `(series_id, issue_id)`
— retry history requires multiple rows per pair — so a duplicate
fire would NOT be schema-absorbed. The explicit
`pending`/`submitted`/`grabbed` check at the top of
`sweep_single_issue` is the only thing preventing a second click on
Search from creating a real duplicate pull_attempt and handing the
same NZB to the downloader twice.

Per-candidate flow extracted as `attempt_pull_for_candidate` —
shared between `sweep_series`'s loop and `sweep_single_issue`'s
one-shot call. `AttemptOutcome` enum drives `sweep_series`'s
pull-list bookkeeping; the single-issue path ignores it.

No PullSearchHandle for per-issue — there's no backend guard,
just the engine's in-flight check. A free function
`fire_issue_search(db, series_id, issue_id)` spawns the task.
Frontend has its own 15s per-row debounce.

`IssueRow.svelte` renders the Search button when `status === 'missing'`
AND a parent seriesId is supplied. Owned, Solicited, Needs-review,
Ignored, Unmatched cases all hide it.

Load-bearing tests both green:
- `sweep_single_issue_skips_when_an_in_flight_attempt_exists` — real
  indexer + downloader wired; engine guard fires BEFORE either is
  called; attempts count for the issue stays at 1.
- `pull_search_issue_works_when_series_is_NOT_on_pull_list` — the
  headline requirement, mirrored at engine and route levels.

## 2. System state

- Container `longbox` is up, healthy in 1s, on the latest build
  (commit `664c054`). Boot tail clean: pool open, Phase B watcher
  started, listening. Zero warn/error lines.
- Phase B watcher is polling `/watch` every 30s. Host-write end-to-end
  validated this session.
- Scanner is preflight-guarded. Next scheduled scan at 03:00 UTC will
  execute normally (library populated); if SMB drops between now and
  then it'll safely 503.
- Pull engine: daily scheduler running, manual `/pull/check` available,
  per-series `/pull/search/:id` available, per-issue
  `/pull/search/:series_id/issue/:issue_id` available. Auto-trigger
  fires on subscribe through both `/pull-list` and the calendar add
  paths.
- Forward-week calendar still serving Metron-sourced rows from the
  morning's Item A v2 cache (24h TTL).
- Docker Desktop VM disk: was at 100% earlier this session; aggressive
  prune freed ~28GB. Worth keeping an eye on across sessions.

## 3. What's open

**Nothing for Phase B core or pull-engine manual control.** Both
closed cleanly. Adjacent items, none urgent:

- `gcd_*` placeholder settings rows from Item A v2 piece 2 are still
  dead-code-flagged. Remove when GCD won't happen.
- Cold-cache Metron forward-week fetch is ~3 min wall time
  (rate-limiter-bound). Operational; pre-warm if it becomes
  user-visible.
- Pre-existing clippy warning in `longbox-comicvine/src/enrichment.rs:437`
  (manual `Range::contains`). One-line fix when next in that file.
- SMB zombie file (`.smbdelete*`) from the morning's Option C probe
  cleanup may still be lurking in
  `/Volumes/Comics/30 Days of Night Falling Sun (2025)/`. macOS-SMB
  artifact, clears on share remount.
- The earlier `docs/longbox-state-of-system-2026-06-01.md` (untracked,
  written before the two pull-engine commits) is superseded by this
  file. Clean up at next pass.

## 4. Codified rules to carry forward

- **Redeploy: `docker-compose up -d --build --force-recreate`.** Every
  time. New workspace crates need the Dockerfile stage-2 COPY line
  added in the same commit.
- **WAL-rule sidecar pattern for live-DB mutations:** stop longbox →
  `docker run --rm --volumes-from longbox alpine` with sqlite
  installed on the fly → start longbox. Used for the morning's two
  cleanup passes.
- **Never run `cargo sqlx prepare` without a properly-migrated live DB
  pointed at by `DATABASE_URL`.** It will silently nuke the `.sqlx`
  cache on validation failures.
- **The container has no `sqlite3` binary.** Alpine sidecar with
  `apk add --no-cache sqlite >/dev/null 2>&1 && sqlite3 -readonly ...`
  for ad-hoc inspection.
- **Live host-write end-to-end probe pattern** (validated three times
  now): build a tiny artifact host-side → drop into
  `/Users/jeremy/longbox-phase-b-watch/` → poll `docker logs longbox`
  for `phase_b.*` → inspect resulting archive on the host via
  `/Volumes/Comics/`.
- **Headless-Chrome via DevTools protocol** is the right tool when a
  bug reproduces in a real browser but not in jsdom (calendar checkbox
  isolation episode).
- **`pull_attempts` has no UNIQUE on `(series_id, issue_id)`.** Retry
  history requires multiple rows per pair. Any code path that submits
  a new attempt MUST guard against in-flight duplicates explicitly —
  the schema won't absorb collisions. The on-demand search paths
  (per-series and per-issue) both do this.

## 5. Standing tone reminder

Jeremy is direct, swears freely, allergic to hedging. State the
answer first. Brevity beats completeness. Don't pad. Don't narrate.
When pushing back, push back hard.
