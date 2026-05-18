# LongBox — Phase B Build Brief

## What Phase B is

Phase B is the bridge between "files exist somewhere" and "files exist
in the library in a state the catalog understands." In practice: a file
lands in the configured download folder (manually dropped, or in the
future deposited by Phase C's downloader integration); Phase B picks it
up, normalizes the filename, writes ComicInfo metadata into the .cbz,
moves it into the right series subfolder under the library root, and
inserts a row into the catalog as `owned`.

Phase B is event-driven. It does not walk the library on a schedule.
It reacts to filesystem events in the watch folder and processes one
file at a time.

Phase B is not Phase C. It does not search indexers, request downloads,
or talk to any external service. It only consumes files that already
exist on disk.

Phase B is "minimum viable." It handles the happy path automatically
and refuses to act on anything ambiguous. Conflict resolution, template
flexibility, multi-source watching, mid-stream renames, and notification
systems are explicitly out of scope and live on the Phase B+ cleanup
queue.

## Phase B "done" definition

Phase B is complete when, against the user's real library and a
configured download folder:

1. A CBZ file dropped into `$DOWNLOAD_WATCH_PATH` is detected within a
   few seconds of arrival (or at startup, if pre-existing).
2. The file's series is identified using the existing matcher (Phase
   A's logic: ComicInfo if present, filename fallback) against the
   already-populated catalog.
3. When identification succeeds: ComicInfo is written into the .cbz
   using catalog data, the file is renamed to the library convention,
   moved into the correct series subfolder, and inserted into the
   `files` table with `status='owned'`, `match_confidence=1.0`,
   `match_method='phase_b'`.
4. When identification fails: the file is moved into `_unsorted/` under
   the library root and inserted with `status='unmatched'`. It surfaces
   in the existing `/files?status=unmatched` view.
5. When the target library path already exists: Phase B refuses to
   overwrite. The incoming file stays in the download folder. A
   "files pending manual intervention" counter on the dashboard
   increments and the file appears in a simple list view.
6. The Phase A.6 "Recently completed issues" activity feed reflects
   newly-imported issues without additional code (it reads the same
   `files.matched_at` timestamp Phase B writes).
7. Container restart while Phase B is mid-process: any
   partially-completed work is recoverable. Pre-existing files in the
   watch folder are picked up by the initial-sweep on startup.

All seven working reliably against the real library + real
SAB-style download folder = Phase B done. Anything more is Phase B+.

## Architecture

### New crate: `longbox-postprocess`

Lives at `crates/longbox-postprocess/` in the existing workspace. Hard
boundaries match the existing pattern.

**Owns:**
- The filesystem watcher (via the `notify` crate)
- The per-file processing tokio task and its serial-consumption channel
- The startup initial-sweep logic
- File I/O for the move (rename or cross-device copy-verify-delete)
- The orchestration of read-zip → write-ComicInfo → write-zip → move

**Depends on:**
- `longbox-core` — matcher, filename parsing, ComicInfo parsing,
  and the new `ComicInfoWriter` (see below)
- `longbox-db` — catalog reads (series + issue lookup) and direct
  inserts via new `files_repo.upsert_imported`
- `notify` crate — cross-platform filesystem events with polling fallback
- `zip` crate — already in tree from Phase A's CBZ extraction; used
  here for writing too
- `tokio` — async runtime

**Does not depend on:**
- `longbox-comicvine` — Phase B never talks to CV. All metadata
  comes from the catalog.
- `longbox-scanner` — Phase B is event-driven, not walk-driven; the
  matcher lives in `longbox-core` so no scanner dependency is needed.
- `longbox-web` — Phase B is invoked by web at startup but doesn't
  depend on web internals.

### Touchpoints in existing crates

- **`longbox-core`** — new module `comicinfo_writer` that takes a
  structured `ComicInfo` value and produces UTF-8 XML bytes suitable
  for embedding in a CBZ. Pure logic, unit-testable in isolation.
- **`longbox-db`** — new migration adds `match_method='phase_b'`
  enum value; new repo method `files_repo.upsert_imported(path,
  series_id, issue_id, size, mtime)`, idempotent on `path_relative`.
- **`longbox-web`** — spawns the postprocess watcher task at startup
  if `DOWNLOAD_WATCH_PATH` is set. Adds dashboard endpoint extension
  returning the "pending manual intervention" count and list. No new
  HTTP endpoints for triggering processing (watch-folder-only trigger
  for v1).

## Decisions (locked during the grill)

### Library convention

Hard-coded, not user-configurable. Phase B always writes to:

```
{library_root}/{series} ({year})/{series} ({year}) {issue:03}.cbz
```

Where:
- `{series}` = canonical CV series name with filesystem-unsafe
  characters (`: / \ ? * " < > |`) replaced with a single space, then
  consecutive whitespace collapsed and trimmed.
- `{year}` = **`series.start_year` (the volume's launch year)**. Never
  `issue.cover_date.year()`. This was a regression vector in the
  previous iteration and is explicitly locked here.
- `{issue:03}` = the issue's `number` field, zero-padded to 3 digits
  if it parses as a pure integer; passed through verbatim otherwise
  (`"½"`, `"Annual 1"`, `"3A"`, etc., stay as-is).

If `series.start_year` is null, the year segment is dropped entirely.
Both folder and filename collapse to `{series}/{series} {issue:03}.cbz`.

Configurable templates (Mylar3-style `folder_format`) are out of
scope. Revisit only if the hard-coded convention proves insufficient
for real use; the existing library is already consistent with it.

### ComicInfo write policy

Always write. Phase B reads the .cbz, generates `ComicInfo.xml` from
catalog data, embeds it (overwriting any existing ComicInfo), then
writes the file in its final location. Field set:

```
Series    — canonical CV series name (catalog)
Number    — issue number, raw string (not zero-padded)
Volume    — series.start_year
Publisher — series.publisher
Title     — issue.title (if non-null)
Year      — issue.cover_date.year   ← issue release date, distinct from Volume
Month     — issue.cover_date.month
Day       — issue.cover_date.day
Web       — CV issue URL
Summary   — issue.summary raw (if non-null). ComicInfo standard
            tolerates HTML in Summary; downstream readers (Komga,
            Kavita, etc.) render it. If it ever looks visually wrong
            in practice, a Rust-side strip becomes a Phase B+ item.
```

`Year`/`Month`/`Day` come from `cover_date`, not `start_year`. They
record when the issue was published; `Volume` records when the series
launched. Both are useful and distinct.

Custom user edits to ComicInfo are out of scope (no UI exists to make
them anyway). If catalog data drifts (rare; CV metadata corrections),
a future "re-tag library" feature can re-write ComicInfo for affected
files. Not Phase B scope.

### Watch folder

- **Single folder**, env var `DOWNLOAD_WATCH_PATH`. Container bind-mount
  expected for real deployments.
- **Recursive walk.** SAB typically creates a per-job subfolder; Phase B
  walks into them, finds the .cbz, processes it, leaves the now-empty
  release subfolder alone (SAB owns post-job cleanup on its own
  schedule).
- **CBZ-only for v1.** CBR support follows whenever Phase A.5's CBR
  task lands (already on the A.5 cleanup queue).
- **Skip patterns:** `*.partial`, `*.crdownload`, `*.!ut`, dotfiles
  (`.DS_Store`, etc.), files whose mtime is within the last 2 seconds
  (avoids reading a file mid-write).
- **Mechanism:** `notify` crate. Auto-detects platform; falls back to
  polling for filesystems that don't support OS notifications (relevant
  if the watch folder is ever SMB-mounted).

### Configuration model

Phase B is enabled implicitly. If `DOWNLOAD_WATCH_PATH` is set and
points to a readable directory, Phase B starts. If unset or unreadable,
Phase B does not start; log a single warning at boot.

No separate `PHASE_B_ENABLED` env var. Configuration drives enablement.

### Conflict handling

When the target library path already exists, Phase B refuses to
overwrite. The incoming file stays in the download folder. Phase B:

1. Logs the conflict via structured tracing (`phase_b.skipped` with
   `reason="conflict"`, `source_path`, `target_path`)
2. Increments a "pending manual intervention" counter visible on the
   dashboard
3. Surfaces the file in a simple list view (linked from the dashboard
   counter): `path-in-download`, `target-path-in-library`, `size`,
   `mtime`. User decides on each: move, delete, rename, whatever.

No hash comparison. No automatic dedup. No "_conflicts/" folder. No
new status enum value. Truly minimum-viable conflict behavior; richer
options live on the Phase B+ queue.

### Unmatched handling

When Phase B receives a file but the matcher cannot identify the series
or issue:

1. The file is moved (not left in download) into `_unsorted/` under
   the library root: `{library_root}/_unsorted/{original_filename}`.
2. A row is inserted into `files` with `status='unmatched'`,
   `match_method='phase_b'`, `match_confidence=null`.
3. The file appears in `/files?status=unmatched` and clusters under
   the `_unsorted/` folder in the By Folder view.

When the user later resolves the unmatched file via Change Match (Task
A's CV search flow) or folder-grouped matching (Task B), the match
resolution endpoint **also moves and renames the file** from
`_unsorted/` to its proper series folder, writing ComicInfo on the way.
The file move is part of the match-resolution path, not just a catalog
update.

This means `_unsorted/` only ever contains truly-unmatched files at
any given moment.

The leading underscore is a deliberate alphabetical-sort hack so
`_unsorted/` clusters at the top of any directory listing. It is not
a hidden-folder convention — the folder is visible to humans browsing
the library directly.

### Catalog insertion

Direct insert, never via the scanner. Phase B owns identification;
the files repo owns persistence; they meet at one new method:

```rust
files_repo.upsert_imported(
    path: &str,
    series_id: i64,
    issue_id: i64,
    size: i64,
    mtime: OffsetDateTime,
) -> Result<File>
```

Idempotent on `path_relative` — re-processing the same file is a
no-op. Sets `status='owned'`, `match_confidence=1.0`,
`match_method='phase_b'`, `is_present=true`, `last_seen_at=now()`,
`matched_at=now()` (the latter feeds Phase A.6's "Recently completed
issues" activity feed automatically).

New `match_method` enum value `'phase_b'` added via migration.

**Failure mode:** if the file move succeeds but the DB insert fails,
the file is in the library uncatalogued. The next full scan will
pick it up via the existing matcher (which will succeed because the
file is now correctly named, located, and ComicInfo-tagged). Not
ideal, but recoverable without data loss.

A future polish item: transactional move-then-insert via temp-path
write + DB transaction + rename-on-commit. Not minimum viable.

### Concurrency

Serial. One tokio task consumes from a channel and processes one file
at a time. Multiple files arriving simultaneously (e.g., SAB completing
a 24-issue release) are queued and processed in arrival order.

This is conservatively slow — 24 files at ~2 seconds each is ~1 minute
of processing. Acceptable for v1. Parallel processing is Phase B+
optimization if/when throughput becomes a real concern.

Bonus property: serial processing eliminates same-target races. Two
files claiming the same library path can't both be in-flight; the
second hits the conflict path cleanly.

### Atomic move

Try filesystem rename first (atomic on same filesystem, instant). If
rename fails with `EXDEV` (cross-device), fall back to:

1. Copy source → target (full byte stream)
2. Verify target matches source (size + byte hash, or just size for v1)
3. Delete source

This is the standard cross-device move. The `notify` crate's events
handle the rename case cleanly. Code internal detail; no user input
needed.

### Failure observability

Single dashboard counter: **"N files pending manual intervention."**
Counter reflects total of:
- Conflicts (target path exists)
- ComicInfo write failures
- Move failures (permissions, disk full, etc.)

Clicking the counter links to a simple list view showing per-file:
- Source path (where the file is now)
- Target path (where Phase B wanted to put it)
- Reason (conflict / write-failed / move-failed)
- Size, mtime
- Last attempted timestamp

No bulk-action UI for v1. User intervenes file-by-file manually.

Phase B does **not** write to the `scan_runs` table. Scans are
catalog-walk events; Phase B is per-file processing. Different
concept, separate audit surface. The dashboard activity feed handles
"what got imported"; the conflict counter handles "what got stuck."

### Logging

Structured tracing events at three levels:
- `phase_b.processed` — file successfully imported. Fields: `path`,
  `series_id`, `issue_id`, `size`, `duration_ms`.
- `phase_b.skipped` — file refused (conflict, unsupported type, etc.).
  Fields: `path`, `reason`, `target_path` (if applicable).
- `phase_b.failed` — processing started and errored mid-stream
  (ComicInfo write failed, move failed, DB insert failed). Fields:
  `path`, `stage`, `error`.

Hooks into the existing `tracing` + `tracing-subscriber` infrastructure
from Phase A. No new logging stack.

### Startup behavior

When the container starts and `DOWNLOAD_WATCH_PATH` is set:

1. Initial sweep: walk the watch folder recursively, build a list of
   eligible CBZ files (passing the skip-pattern filter), enqueue them
   on the processing channel in arbitrary order.
2. Attach the `notify` watcher to the watch folder for subsequent
   filesystem events.
3. Start the processing task: pulls from the channel, processes one
   at a time.

Pre-existing files (left in the watch folder from before container
start) get processed via step 1. New files arriving after step 2 get
queued via the notify watcher. No file is missed regardless of
arrival timing.

## Data model

### Schema changes

**New migration adds:**
- `match_method` enum gains `'phase_b'` as a valid value
- `files.matched_at` (DATETIME NULL) — set when a file transitions to
  matched status (owned, needs_review, or ignored after manual action).
  Feeds Phase A.6's "Recently completed issues" activity feed.

**No new tables.** The conflict/failure visibility surface reads from
filesystem state (download folder contents + library state) and a
small in-memory cache; no persistent table for v1.

### New types

In `longbox-core`:

```rust
pub struct ComicInfoWriter { ... }
// Takes structured ComicInfo data, produces UTF-8 XML bytes.

pub struct LibraryPath {
    series_name: String,
    start_year: Option<i32>,
    issue_number: String,
}
impl LibraryPath {
    pub fn folder(&self) -> String { ... }
    pub fn filename(&self) -> String { ... }
    pub fn full(&self, library_root: &Path) -> PathBuf { ... }
}
// Encodes the convention. Pure logic, exhaustively tested.
```

In `longbox-postprocess`:

```rust
pub struct PostprocessConfig {
    pub watch_path: PathBuf,
    pub library_root: PathBuf,
}

pub async fn start(config: PostprocessConfig, db: DbPool) -> Result<()>;
// Entry point called from longbox-web at startup.

pub struct PendingIntervention {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub reason: InterventionReason,
    pub size: i64,
    pub last_attempt: OffsetDateTime,
}
pub enum InterventionReason {
    Conflict,
    ComicInfoWriteFailed(String),
    MoveFailed(String),
}
```

The pending-intervention list lives in an in-memory cache (`Arc<RwLock<
Vec<PendingIntervention>>>`) shared with `longbox-web` for the dashboard
endpoint to read.

## UI changes

Phase B itself introduces no new pages. It surfaces through existing
and Phase A.6 surfaces:

- **Dashboard counter row**: "N files pending manual intervention"
  (alongside the other counter tiles). Links to a simple list view.
- **Pending-intervention list view**: simple table at
  `/files/pending-intervention`. Path, target, reason, size, mtime.
  No bulk actions for v1.
- **Activity feed** (Phase A.6 Task 3): "Recently completed issues"
  surfaces Phase B's writes automatically via `files.matched_at`. No
  Phase-B-specific UI.

## HTTP API

Two small additions:

- `GET /api/postprocess/pending` — returns the in-memory
  pending-intervention list. Used by the dashboard counter and the
  list view.
- (No HTTP trigger for processing in v1. Phase C adds
  `POST /api/postprocess/process` with explicit metadata when the time
  comes.)

## Out of scope (Phase B+)

- Hash-based dedup on conflict (the "incoming file is byte-identical
  to existing target, silently delete incoming" optimization)
- Dedicated `_conflicts/` folder + `conflict` status
- Bulk resolution UI for pending interventions
- Template-driven library structure (configurable folder/filename
  format)
- Multi-folder watch
- Runtime reconfigurable watch folder (UI-driven)
- CBR support (already on the A.5 cleanup queue)
- Parallel processing / worker pool
- Transactional move-then-insert with rollback
- Re-tag library on catalog drift (re-write ComicInfo for affected
  files)
- Notification system (email / Slack / webhook on conflict)
- HTTP push API for Phase C metadata pass-through
- Sidecar `.longbox.json` metadata files
- ComicInfo write opt-out per-series or globally
- Custom user-edited ComicInfo preservation across re-tag

## Build approach

Suggested step decomposition. One commit per step. Kickoff discipline
applies: surface ambiguity at each step's start before writing code.

1. **`comicinfo_writer` in `longbox-core`** — pure logic, no I/O. Unit
   tests with fixture data covering happy path, missing fields,
   special characters, the v1 field set. Lands first; nothing depends
   on Phase B yet.

2. **`LibraryPath` type in `longbox-core`** — encodes the convention.
   Unit tests for filesystem-unsafe character handling, integer
   zero-padding, non-integer issue numbers, missing-year fallback.
   Pure logic.

3. **Migration + `files_repo.upsert_imported` + `files.matched_at`
   in `longbox-db`** — schema change, new repo method. SQL-level
   tests. Updates the `match_method` enum to include `phase_b`.

4. **`longbox-postprocess` crate skeleton** — workspace integration,
   config type, entry point stub. No watcher or processing logic yet.
   Verify the crate compiles and links.

5. **Initial sweep + notify watcher** — file detection only. Logs
   what it would process; doesn't actually process. Verify against
   a real download folder that the right files are identified and
   the skip patterns work.

6. **Processing pipeline** — wires the matcher, ComicInfo write,
   library convention, file move, catalog insert. The big step.
   Real-library verification: drop a CBZ into the watch folder, see
   it appear in the catalog correctly.

7. **Conflict + failure surface** — pending-intervention cache,
   dashboard counter, list view. Test by intentionally creating a
   conflict and verifying it surfaces.

8. **Acceptance smoke against real workflow** — drop 5+ files of
   varying provenance into the watch folder; verify all 5 are
   either correctly catalogued or correctly surfaced as pending.

## Standing rules

(Carried over from Phase A — re-stated for the Phase B context.)

- Hard crate boundaries. `longbox-postprocess` does not leak its
  internals; `longbox-web` calls one entry-point function.
- Errors are typed (`thiserror` per crate); no `anyhow` in library code.
- `.sqlx/` offline cache regenerated for any commit that touches SQL.
- `cargo fmt --check` + `cargo clippy -- -D warnings` clean at every
  commit.
- All public functions documented with `///` rustdoc.
- All structured tracing events use snake_case field names matching
  Phase A's convention.

## Kickoff discipline

(Same shape as Phase A.5.)

Each step starts with: surface ambiguity, stop, wait for explicit
"proceed." No chaining without explicit go-ahead. Each step ends with:
build, run tests, manual verify the slice, stop, wait for proceed on
the next step.

For Phase B specifically: the real-library verification on step 6 is
non-negotiable. A green unit-test suite without a successful
end-to-end drop-a-file-watch-it-land confirmation is not done.

## Phase B+ cleanup queue

Items deferred during Phase B, captured for future reference. Pick up
at discretion.

(See "Out of scope" above for the full list — those entries graduate
to this queue as Phase B+ work as they become relevant.)
