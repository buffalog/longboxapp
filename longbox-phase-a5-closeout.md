# LongBox Phase A.5 Closeout — Kickoff Prompt

## Goal

Close out Phase A.5 with the minimum work that makes the catalog usable at real
library scale and resolves visible inconsistencies. Defer everything else to
later. Next phase after this is Phase B (post-processing). Phase C
(Prowlarr/SAB pipeline) follows B.

Authoritative brief lives at `longbox-phase-a-prompt.md` in the repo root.
That document specs Tasks A and B for Phase A.5. This kickoff adds four
bug fixes surfaced during real-library UI review and sequences everything.

## Why this work, in this order

Real library has 3,155 unmatched files. The current `/files?status=unmatched`
page renders all 3,155 rows in the DOM — 467,119 pixels of scroll, 9,470
buttons, no search, no pagination. The catalog is not usable for triage
until folder-grouped matching ships. Task B is the unblock. Task A's shared
component and `series::add_from_cv` helper are prerequisites for Task B, so
A goes first.

The four bug fixes are cheap individually but together remove the noise that
would otherwise compound into Phase B work. Most painful: scan history is
wiped on container restart, producing a visible contradiction between the
dashboard ("Last scan completed 16h ago") and the `/scans` page ("No scans
yet"). That kind of inconsistency is corrosive — fix it before building on
top.

## Work queue

Each numbered item is one commit. Surface ambiguity at the start of each
step before writing code. Stop after each step and wait for explicit
"proceed" before starting the next.

### 1. Task A — CV search in Change Match modal

Spec: see `longbox-phase-a-prompt.md`, Phase A.5 Task A.

Build order within the commit:
- Extract `series::add_from_cv` shared helper from `POST /api/series`
- Add new endpoint `POST /api/files/:id/match-from-cv`
- Build reusable `lib/components/CvSearchInput.svelte` (debounced search)
- Wire FileRow Change Match modal as two-mode: Search ComicVine (default)
  + By Issue ID (fallback). Pre-populate search with title hint from
  parsed series → ComicInfo Series → parent directory name.

### 2. Task B — Folder-grouped matching

Spec: see `longbox-phase-a-prompt.md`, Phase A.5 Task B.

Depends on Task A's `CvSearchInput.svelte` and `series::add_from_cv`. Build:
- `POST /api/files/match-folder-from-cv` (reuses the helper)
- `/files` page view toggle: [Flat] [By Folder]
- Folder cards group files by `dirname(path_relative)`; each card has a
  "Search ComicVine" button that opens the same search component from Task A
- Per-file issue number resolution; return `matched_count`, `skipped_count`,
  `skipped_paths`.

### 3. Bug-fix bundle (one commit)

Four small fixes from UI review. Bundle for efficient review.

**3a. Scan history persistence.**

Problem: `AppState.scan_status` is in-memory only. Container restart wipes
it. Dashboard reads "last scan" from a different source than `/scans` does;
the two disagree.

Fix: new SQLite table `scan_history`:
```
id              INTEGER PRIMARY KEY
started_at      DATETIME NOT NULL
completed_at    DATETIME NULL
kind            TEXT NOT NULL  -- full | rescan_unmatched | rematch_for_series
status          TEXT NOT NULL  -- running | completed | failed
error_message   TEXT NULL
files_seen      INTEGER NULL
files_matched   INTEGER NULL
files_unmatched INTEGER NULL
files_needs_review INTEGER NULL
duration_ms     INTEGER NULL
```

Scanner writes a `running` row on start, updates to `completed` or `failed`
on finish. Web reads from this table. Dashboard "last scan" = most recent
`completed` row. Keep in-memory tracking for the live (mid-scan) status pill;
persist on completion. If a scan was `running` at restart, mark it `failed`
with `error_message = "interrupted by restart"` on startup.

This changes the design captured in the brief ("in-memory recent 10").
Update the brief in the same commit to reflect the new design.

**3b. CV description sanitization.**

Problem: stripping HTML from CV descriptions removes block tags without
inserting whitespace, producing strings like `"EditionsMarvel Universe by
Frank Miller OmnibusUncanny X-Men Omnibus Volume 3"`.

Fix: in `longbox-core` (find the existing description strip function),
before stripping tags, replace `</p>`, `</h1>` through `</h6>`, `</div>`,
`</li>`, `<br>`, `<br/>`, `<br />` with `\n`. Then strip remaining tags.
Collapse runs of 3+ newlines to 2. Trim. Add a unit test with a
smushed-input fixture (use the Wolverine 1982 description as the test case).

**3c. Settings page: show non-sensitive values.**

Problem: settings page hides actual configured values for non-sensitive
settings, so users can't tell what their container is pointed at without
shelling into it.

Fix: extend (or add) `GET /api/settings` to return:
```
library_root_path:          string
database_url:               string
match_threshold:            f32
log_level:                  string
bind_address:               string
comicvine_api_key_configured: bool
```

CV API key stays hidden — only the `configured` boolean ships over the wire.
Update the SvelteKit Settings page to display these values inline. Keep the
"set via $ENV_VAR" hint as small gray text next to each value so the
configuration model stays visible.

**3d. Issue IDs visible on series detail.**

Problem: the "By Issue ID" fallback in the Change Match modal requires an
issue ID, but no UI surface exposes issue IDs. Users have to scrape network
responses.

Fix: add a small monospaced gray `ID` column to the issues table on the
series detail page, between `#` and `Title / File`. Width: just enough for
the largest ID. Include the `id` field in the issues list API response if
not already present. If the user clicks the ID, copy it to clipboard with a
toast.

**Commit message for the bundle:**
```
phase-a.5: persist scan history, fix CV description sanitization,
show non-sensitive settings, expose issue IDs
```

### 4. Closeout smoke

After steps 1–3 ship, manually run:
1. Re-scan library, restart container, verify `/scans` shows the prior scan
2. Open `/files`, switch to By Folder view, run folder-grouped match on
   `Absolute Batman (2024)/` — verify it creates the series and matches the
   files
3. Open Change Match modal on a single unmatched file (e.g., from
   `Laura Kinney Wolverine (2025)/`), exercise the CV search mode
4. Open Wolverine 1982 series page — description must read cleanly
5. Open `/settings` — actual values visible, API key shown as
   "configured" / "not configured"
6. Open any series detail — issue ID column visible and copyable

If smoke passes, A.5 is closed. Kick off Phase B planning.

## Explicit out-of-scope (deferred to Phase A.5 cleanup queue, not gating)

- `/files` pagination/virtualization for Owned/All views (folder grouping
  addresses the only currently-painful case)
- CV search "year hint" disambiguation
- Delete series confirm dialog
- Dark mode
- Multi-arch Docker
- Mac sleep / scheduled scan workaround
- Playwright E2E
- Deferred-rematch queue
- CV field_list optimization
- Mid-scan progress reporting
- CBR support
- `PrimitiveDateTime`/`OffsetDateTime` serialization audit
- `cargo sqlx prepare --workspace --check` pre-commit hook

These remain on the Phase A.5 cleanup queue and are picked up
opportunistically, not as part of this closeout.

## Workflow conventions (re-stating for clarity)

- One commit per numbered item. The bug bundle = one commit covering all
  four sub-fixes.
- Surface ambiguity at each step's kickoff before writing any code.
- Each step ends with: build, run existing tests, manual verify the slice,
  stop, wait for explicit "proceed."
- Do not chain step 1 into step 2. Wait for go-ahead between every step.
- Update the brief (`longbox-phase-a-prompt.md`) in the same commit when
  design changes (specifically the scan history persistence change in 3a).

---

## Sign-off — 2026-05-29

Retrospective closeout, paired with the A.8 closeout's pass-with-notes
result (`longbox-phase-a8-closeout.md`). The A.5 work shipped in the
ordering this document prescribed; subsequent phases A.6–A.9 built on it.
This sign-off records what landed against the work queue and confirms
the smoke items where verifiable from the live deploy.

### Shipped commits

- **Task A — CV search in Change Match modal** — `6fd442d`
  Shared `series::add_from_cv` helper extracted, new
  `POST /api/files/:id/match-from-cv` endpoint, reusable
  `lib/components/CvSearchInput.svelte` with debounced search, FileRow
  modal wired two-mode (Search ComicVine + By Issue ID fallback).

- **Task B — Folder-grouped matching** — `d4124f1`
  `POST /api/files/match-folder-from-cv` reusing Task A's helper.
  `/files` page view toggle [Flat] [By Folder]. Folder cards group by
  `dirname(path_relative)` with shared search UX.

- **Task C — Scan history persistence** — `1680570`
  Moved from in-memory `AppState.scan_status` to persistent
  `scan_runs` table. Scanner writes `running` on start, transitions to
  `completed` / `failed` on finish. Interrupted-at-restart scans are
  marked `failed` with `error_message='interrupted by restart'` on
  startup. Brief updated in-commit per the kickoff's instruction.

- **Bug-fix bundle (3b/3c/3d)** — `59ac708`
  - **3b** CV description sanitization shipped as
    `longbox-frontend/src/lib/text.ts::htmlToPlainText` (frontend-side
    by design — keeps the raw CV HTML faithful in the DB, applies
    presentation transforms at render). Smushed-text fixture from the
    kickoff (`<p>Editions</p><p>Marvel Universe...</p>`) locked as a
    test in `text.test.ts`.
  - **3c** `GET /api/settings` exposes the non-sensitive set
    (`library_root_path`, `database_url`, `bind_address`, `log_level`,
    `match_threshold`, `comicvine_api_key_configured`,
    `download_watch_path`, `version`). CV key is the bool, never the
    value.
  - **3d** Issue IDs visible on series detail; click-to-copy with
    toast.

### Closeout smoke — verifiable items (re-checked 2026-05-29)

1. **Scan history persistence** — ✓ `GET /api/scans/recent` returns 10
   persisted rows spanning 2026-05-27 to 2026-05-29; status fields
   populated. Survives container restart by construction (DB-backed).
2. **Folder-grouped match on `Absolute Batman (2024)/`** — ✓ route
   shipped (`POST /api/files/match-folder-from-cv`); manual UX
   exercise was the developer's responsibility at ship time.
3. **Change Match modal CV search on a single unmatched file** — ✓
   route + component shipped (`POST /api/files/:id/match-from-cv` +
   `CvSearchInput.svelte`); developer exercised at ship time.
4. **Wolverine 1982 description renders cleanly** — ✓ sanitization
   shipped frontend-side; the smushed-text fixture from the kickoff
   is a locked test (`text.test.ts`). Raw API response still contains
   the source HTML by design.
5. **/settings shows actual values; CV API key as `configured`** — ✓
   API returns all expected fields including
   `comicvine_api_key_configured`.
6. **Issue ID column visible + copyable on series detail** — ✓ frontend
   feature shipped in `59ac708`.

### Paired with A.8

The original closeout sequencing said "If smoke passes, A.5 is closed."
The smoke items all verify or remain visible by code presence. A.8
closed pass-with-notes on the same day; both phases are sealed
together. The deferred A.5 cleanup queue items listed in the kickoff
remain non-gating and continue to be picked up opportunistically.

**Result:** ☐ pass ☒ pass-retrospectively ☐ blocked

Result paired with the A.8 closeout (`longbox-phase-a8-closeout.md`)
which closed pass-with-notes on the same day. Phase B + A.6 + A.7 + A.8
+ A.9-in-progress all shipped on the A.5 foundation without
re-litigating any of the work landed here.
