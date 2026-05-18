# LongBox — Phase A.6 Plan (Polish & Discovery)

## Read this first

This is forward planning, not an immediate kickoff. Code is currently
executing Phase A.5 closeout (Task D in flight; E, F pending). This
document captures the next phase after A.5 closes, sequencing toward
Phase C.

## Goal

Phase C is the destination: Prowlarr indexer integration, SAB downloader
integration, automated retry on failed downloads. Full Mylar replacement.

## The sequence

1. **A.5 closeout** (in progress) — Tasks D, E, F land. A.5 closes.
2. **A.6 polish + discovery** (this document) — bug fixes from real-library
   use plus three new user-facing features. Bridges "catalog works" to
   "catalog is pleasant to use."
3. **Phase B** — post-processing pipeline. Watch folder → renamed/tagged →
   moved into library. The bridge between downloads (Phase C) and the
   catalog (Phase A).
4. **Phase C** — Prowlarr + SAB. The destination.

Phase B is the gate to C because without post-processing, downloads land
in a folder the catalog doesn't know how to integrate. Phase B can be
minimal (file-move + ComicInfo write) or rich (template-driven rename
schemes, conflict resolution, partial-file handling). Minimum viable
Phase B is enough to unlock Phase C; full Phase B is more.

## Phase A.6 work queue

Numbered and sequenced. One commit per task unless noted. Same workflow
conventions as A.5: surface ambiguity at each step's kickoff before
writing code; stop after each step and wait for explicit "proceed."

### 1. `/scans` visual fixes (bundle)

Three small fixes surfaced during real-library use:

**1a. Scan card grid layout.** Labels and values are visually misaligned —
the CSS grid is putting label/value pairs in the wrong cells, making
each card hard to parse. Inspect the existing layout, fix so each counter
is a clear `Label: Value` pair, three pairs per row.

**1b. Sub-second duration display.** A no-op rescan currently renders as
`0ms`, which reads as a bug. Show `< 1s` for sub-second durations, or
hide the duration field entirely when it would be sub-second. Pick
whichever fits the card design better.

**1c. Dashboard "Last scan" semantics.** Dashboard currently shows
"Last scan completed Xm ago" referencing whatever's most recent in
`scan_runs`, which includes failed and no-op rescans. Change the
backing query to "most recent successfully-completed scan that did
real work" — successful AND (`files_seen > 0` OR `kind = 'full'`).
If no such scan exists, display "Never scanned" or "No scans yet."

**Commit:** `phase-a.6: /scans visual fixes (grid, sub-second duration, dashboard last-scan)`

### 2. `/files` By Folder filter input

With 705 folder cards in the real library, navigation needs a search
input. Pure client-side substring filter on folder name; no backend
change.

**Frontend:** add a text input at the top of `/files` page, visible only
when the [By Folder] toggle is active. Case-insensitive substring match
on folder name. Filter persists in URL params (`?folder_filter=saga`)
so filtered views are shareable. Clears when toggling back to Flat
view.

**Commit:** `phase-a.6: folder-name filter on /files By Folder view`

### 3. Dashboard activity feed

Two new sections below the counter tiles on the dashboard.

**Recently added series** — last 6 series by `series.created_at`. Card:
cover thumbnail, name, year · publisher, owned/total badge. Click → series
detail page.

**Recently completed issues** — last 6 issues that became matched to a
file. Surfaces "what's new in your library." Card: issue cover thumbnail,
series name + issue number, filename, relative time ("3 hours ago").

**Backend:**
- Verify `series.created_at` exists; should from Step 2.
- Verify (or add) a "matched at" timestamp on files. Either
  `files.matched_at` as a new column (set when matcher assigns issue),
  or use the existing `files.updated_at` filtered to status transitions
  into `owned`. Adding a dedicated column is cleaner.
- New endpoint `GET /api/dashboard/activity?limit=6` returning both
  lists in one response.

**Frontend:** extend dashboard page below the counter tiles. Two
two-column-wide sections side by side on desktop, stacked on mobile.

**Commit:** `phase-a.6: dashboard activity feed (recently added series + recently completed issues)`

### 4. International reprint filter

The case: user types "Batman" in `/add`. CV returns DC Comics original
AND Panini French reprint AND Planeta DeAgostini Spanish reprint AND
ECC Ediciones — distinct CV volumes for the same underlying content,
in different languages. User wants the DC original, not the reprints.
But the filter must not blanket-exclude small foreign-based-but-original
publishers (Oni Press, etc.). Publisher-name blocklist with curated
defaults is the right shape.

**Backend:**
- New migration: table `publisher_filters` with columns `id` (PK),
  `publisher_name` (TEXT NOT NULL UNIQUE, case-insensitive),
  `mode` (TEXT NOT NULL CHECK in `('block', 'allow')`), `created_at`.
- Seed migration with a curated default blocklist (names verified
  against CV's actual records during Task 4 implementation): Panini
  Comics, Panini France, Panini Brasil, Planeta DeAgostini, Editorial
  Televisa, ECC Ediciones, Norma Editorial, Éditions Glénat, Arnoldo
  Mondadori Editore, Salvat. Add others as discovered.
- New endpoints: `GET /api/publishers/filters`, `POST
  /api/publishers/filters` (with body `{publisher_name, mode}`),
  `DELETE /api/publishers/filters/:id`.
- CV search endpoint filters results where publisher matches any
  `block` filter (case-insensitive). Filters apply by default.

**Frontend:**
- `/settings` gains a "Publisher filters" section. Lists current
  filters; allows add / remove. Curated defaults shown as removable
  with a "reset to defaults" affordance.
- `/add` CV search results respect filters by default.
- `/add` has a "Show filtered results" checkbox below the search input
  for one-off overrides (useful when search itself looks suspect, or
  when investigating whether a result was incorrectly filtered).

**Design notes:**
- Blocklist is publisher-NAME-keyed, not publisher-ID-keyed. Names are
  more stable across CV data drift than IDs.
- Case-insensitive comparison. Store as user entered; compare normalized.
- The `allow` mode exists but is unused by the default seed. Future-
  proofs against a "allowlist mode" where users could opt into an
  exclusive set of publishers. Not exposed in UI yet.

**Commit:** `phase-a.6: international reprint filter (publisher blocklist + CV search filter)`

### 5. Completionist view

First step toward the "suggestions" feature. The full reading-history-
based recommendation engine requires reader integration (Komga / Kavita
/ etc.), which is post-Phase-C scope. The achievable near-term down-
payment is showing what's missing.

Three additions:

**5a. `/series` sort by completion.** Add a sort dropdown to the series
list page: name (default) | year | added | completion %. Completion %
surfaces "almost complete" series at the top — natural prioritization
signal for what to acquire next.

**5b. `/missing` page.** New page listing missing issues across the
library. Columns: cover thumbnail (if CV has one), series name, issue
number, cover date, "missing for X" relative age (cover_date vs today).
Filter by series. Sort by series (default) or cover date.

**5c. Dashboard missing card.** Add a bottom row to the dashboard:
"X issues missing across Y series · view all" linking to `/missing`.

**Backend:**
- No new tables. Use existing `issues` and `files` data.
- New endpoint `GET /api/missing` with optional `?series_id=N` filter.
- Series list endpoint already exposes owned/total; sort by completion %
  is a query change.

**Frontend:** new page + sort dropdown + dashboard card.

**Commit:** `phase-a.6: completionist view (series sort, /missing page, dashboard card)`
(or split as 5a / 5b+5c if Code prefers two commits.)

### 6. Needs Review triage workflow

Surfaced during real-library use: actioning a file on `/files?status=needs-review`
(Accept Match, Change Match, Mark Ignored) doesn't remove the row, breaking
the triage rhythm. Fixing this opens the door to a proper keyboard-driven
triage workflow.

**Visual transition.** After any status-changing action succeeds on a row:

1. Row shows a green checkmark / "Matched" (or "Ignored") overlay for ~300ms
2. Row slides or fades out over ~200ms
3. Subsequent rows shift up
4. Live pill count ("Needs review (N)") decrements

Total ~500ms. Optimistic — frontend removes the row immediately on click,
restores on backend failure with an error toast. The brief success state
confirms action registered without slowing triage rhythm.

**Keyboard shortcuts.**

- `a` — Accept Match (only enabled when a match suggestion exists)
- `m` — Change Match (opens CV search modal)
- `i` — Mark Ignored
- `j` — focus next row
- `k` — focus previous row
- `?` — show shortcuts reference modal
- `Esc` — close any open modal

Single-key shortcuts fire only when focus is on the page body — modal
dialogs (Change Match's CV search) capture their own input, so typing
in the search field doesn't fire `a`/`m`/`i`.

**Focus management.** After a successful action, focus jumps to the next
row's primary action button (Accept Match if a match suggestion exists,
otherwise Change Match). Maintains rhythm: read content → press key →
next content appears → repeat. When acting on the last row, the list goes
empty with the existing "Nothing needs review" empty state.

**Current row indicator.** Subtle background tint on the row containing
the focused button, paired with the browser's native focus ring on the
button itself. Keyboard users get a clear "I'm here" marker; mouse users
see only the focus ring on hover (status quo).

**Discovery.** Two surfaces:
- Always-visible hint line below the page header in small grey text:
  `a accept · m match · i ignore · j/k navigate · ? for help`
- `?` opens a modal listing all shortcuts with descriptions

**Scope.** Applies to `/files?status=needs-review` only. Unmatched's
primary workflow is the folder-grouped modal (Task B); j/k navigation
across the flat list (~2,998 files in the real library) is not
practical there. The Unmatched page keeps its mouse + folder-card
workflow. Owned and Ignored pages don't get keyboard shortcuts — those
files are already in their final state. The folder-card "Search
ComicVine" shortcut (`s`?) is deferred to A.7+.

**Backend:** no changes required. All existing endpoints already return
the data needed; the work is entirely frontend.

**Frontend:** rework `FileRow.svelte` to handle optimistic remove + success
animation. Add page-level keyboard event listener with focus-tracking
state. Add hint line component + shortcuts modal. Update the file list
container to manage focus transitions after row removal.

**Commit:** `phase-a.6: needs-review triage workflow (optimistic remove, keyboard shortcuts, focus management)`

## Suggested execution order

1 → 2 → 6 → 3 → 5 → 4

Save the publisher filter (4) for last because it touches the most
surfaces (new table, migration, two endpoint families, settings UI,
search filter, override checkbox). It benefits from a cleaner A.6 base
and there's no inter-task dependency forcing it earlier.

1 and 2 are small visual fixes that ship fast. 6 (triage workflow) goes
next because it's a daily-use improvement against pages you're actively
working in right now — front-loads the win. 3 and 5 are new features that
build on the polished foundation. 4 closes out A.6 with the highest-surface
work.

Code may bundle 1 and 2 as a single commit if the diff stays small.

## Out of scope (explicitly deferred)

- **Reading-history-based recommendations.** Need reader integration.
  Post-Phase-C.
- **Multi-language / unicode-normalized publisher matching.** The
  curated English-name list handles the cases that matter; non-ASCII
  edge cases (publishers with diacritics in their canonical names) can
  be handled via the user-editable filter list.
- **Per-series "ignore this publisher" override.** Use existing
  per-series ignore status (already supported). Don't over-engineer.
- **"Mark series as read" / "currently reading" workflows.** Reader
  integration territory.
- **Subscription / notification on new issues.** Phase C territory
  (Prowlarr already does feed monitoring).
- **Per-issue file preview / inline reader.** Out of scope per the
  brief's stated indefinite exclusions.

## Phase B and Phase C

Phase B has been grilled and a brief exists at
`longbox-phase-b-brief.md`. Scope locked at "minimum viable" (the smart
bridge: watch folder → matched + ComicInfo-written + moved into library
→ catalog insert). Phase B has its own architectural decisions captured
in spec form, similar shape to the Phase A brief.

Phase C (Prowlarr + SAB) follows Phase B and is not yet briefed.

The estimated path from current state to Phase C kickoff is:

- A.5 closeout: 3 more commits (Tasks D, E, F)
- A.6: 6 commits (this document, now including the triage workflow)
- Phase B execution: 8 steps per its brief, one commit each
- Phase C planning + grilling
- Phase C execution

A.5 + A.6 are mechanically straightforward. Phase B is substantial but
the design is locked. Phase C is where the next real design grilling
happens — indexer integration, downloader integration, retry policy,
release selection logic.
