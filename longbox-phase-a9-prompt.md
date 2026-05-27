# LongBox Phase A.9 — feature queue from the dogfood pass

Eleven improvements surfaced while dogfooding the A.8 release surfaces
and Library Tidy. This brief is the canonical A.9 plan; per the A.8
precedent it is amended as each step resolves.

**Convention:** each step gets a kickoff (numbered implementation
questions, each with a recommendation) → approval → a single
implementation commit → container rebuild + redeploy.

**Status (2026-05-24):** Steps 4, 6a and 6b shipped; four hot-fixes
shipped — the 6a bulk-convert dedup hot-fix, the parser hot-fix
(three new patterns covering `Series (YYYY) NNN`,
`Series N - Subtitle (YYYY)`, and `Series N (Xf Y) (YYYY)`), the
F6 dismiss-trap hot-fix (split `discovered_folders.dismissed_at`
into user-permanent vs auto-dismiss columns), and the
shallow-series UX hot-fix (hide CV-only Refresh affordance + adjust
empty-issues hint when `cv_id` is NULL). Steps 1–3, 5, 7 and 6c
queued. The A.8 closeout smoke (`longbox-phase-a8-closeout.md`,
Step 13 of the A.8 brief) was staged, paused four times for
hot-fixes, and resumes once the user finishes bulk-converting the
~216 untracked folders surfaced by the F6 fix.

---

## Step structure

A.9 is seven steps (Step 6 split three ways), grouped by surface,
ordered by dependency and impact.

### Step 1 — UI polish sweep · items 4, 11

Low-risk warm-up. New-tab links (item 4) and the three dashboard polish
fixes (item 11). No schema, no ComicVine calls. Kickoff was surfaced and
all questions approved; implementation deferred in favour of Step 4.

### Step 2 — Calendar solicitation completeness · item 8

The most-misleading live surface — the calendar shows one issue for a
future week when hundreds are expected. `cover_date`-offset fallback for
null `store_date`, plus a far-term UI note (item 8).

### Step 3 — Publisher grouping on the calendar · item 1

Per-volume cached ComicVine resolution; reopens Step 8 Q1 (per-row
resolution was infeasible at 180/hr, per-volume cached resolution is
tractable). Sequenced after Step 2 — Step 2's CV-query archaeology
informs Step 3's per-volume cache design.

### Step 4 — Bulk add-to-pull-list + of-note badge · items 6, 7

Checkbox-multiselect bulk add on the calendar and the of-note widget
(item 6); the of-note widget shows an "On pull list" badge in place
instead of removing rows after add (item 7). Factored as two reusable
primitives — a `BulkActionBar` component and a `createSelection` helper
— so Step 6 inherits them rather than building parallel.

### Step 5 — Scheduling & timezone · items 3, 5

Scan frequency as a configurable interval (item 3); timezone made
configurable on two axes — UI display and scheduler (item 5).

### Step 6 — Library Tidy accuracy & UX

Step 6 split into three sub-steps at its kickoff (the bulk-convert RC
blocker is independent of the rest, and CV enrichment is its own design
surface).

**Step 6a — bulk-convert (RC blocker).** Shallow-convert discovered
folders → tracked series with no ComicVine: each folder becomes a series
(title + launch year parsed from the folder name) *plus number-only
issue rows synthesized from its files' parsed filenames* — so the files
attach as `owned` and catalog counts are correct immediately. (Title +
year alone would leave the files unmatched — it needs the synthesized
issues; see the deferred-items note's reasoning.) Also refactors
`/library/tidy`'s two hand-rolled bulk sections onto Step 4's
`BulkActionBar` + `createSelection`.

**Step 6b — auto-tidy + phantom UX · items 2, 10.** Auto-tidy on folder
removal (item 2): N=3 consecutive empty scans, soft-delete with a
recovery window, a new `series.consecutive_empty_scans` column. Phantom
UX (item 10): split the zero-ownership bucket into "Awaiting first
download" (on the pull list / has a pull_attempt) vs "Empty series".

**Step 6c — CV enrichment.** Backfill CV metadata (covers, descriptions,
the canonical issue list) for shallow-converted series. Its own kickoff:
the scan-scheduler is a daily timer and can't host it, and a shallow
series has no `cv_id`, so enrichment must CV-*search* + auto-pick — a
real design surface (confidence threshold, manual fallback).

### Step 7 — Missing-issue resolution · item 9

Backissue search — the biggest item. `/missing` gains a resolution
path. Likely splits into 7a (per-row search) and 7b (bulk).

### Step 7 — Missing-issue resolution · item 9

Backissue search — the biggest item. `/missing` gains a resolution
path. Likely splits into 7a (per-row search) and 7b (bulk).

---

## The 11 items

1. **Publisher grouping on the calendar.** Sectioned headers (DC Comics,
   Image, Marvel, …), titles alphabetical within each section, an
   "Unknown" bucket for cache misses during lazy-fill. Per-volume cached
   resolution — volume-static, persistent, cold-start tax only. → Step 3

2. **Auto-tidy on folder removal.** When a tracked series's folder
   disappears from disk, auto-delete the series record without manual
   `/library/tidy` confirmation. Debounce: N=2–3 consecutive scans
   showing the folder absent before removal, to absorb mount blips. A
   setting, defaulted on. → Step 6b

3. **Scan frequency.** 8 scans per 24h (every 3 hours). Interval-based
   scheduling sidesteps timezone entirely. Configurable via settings. → Step 5

4. **Calendar / of-note links → new tab.** `target="_blank"
   rel="noopener noreferrer"` on the ComicVine `site_detail_url`
   volume-name links across the calendar table and the of-note widget. → Step 1

5. **Timezone configurable.** Two axes: (a) UI display TZ — a user
   setting; the frontend formats UTC timestamps in the preferred zone
   (PST default). (b) Scheduler TZ — config-time UTC conversion via
   `chrono-tz`, re-resolved on DST transitions. → Step 5

6. **Bulk "Add to pull list".** Checkbox per row + an "Add N selected"
   button on the calendar and the of-note widget. New
   `POST /api/releases/calendar/pull/bulk` taking `{ cv_volume_ids }`,
   non-transactional, returning a 3-way per-item status (added /
   already_on_list / failed) surfaced as one aggregate toast. → Step 4

7. **Of-note widget UX.** Stop removing rows on add — show an "On pull
   list" badge in place (the calendar's existing emerald pill). The
   widget stays visible so the user sees their action land. → Step 4

8. **Calendar solicitation completeness.** ComicVine's `store_date` is
   sparsely populated for future weeks, so the calendar under-reports.
   Patterns: a `cover_date`−offset heuristic fallback for null
   `store_date`, a different CV query path, and/or a UI note that
   solicitation data thins beyond N weeks. → Step 2

9. **Missing-issue resolution (backissue search).** `/missing` is
   informational-only with no resolution path. Add a manual trigger:
   per-row "Search" → a result modal of release candidates → submit as a
   `pull_attempt` with a new `MatchMethod::ManualSearch` (distinct from
   `PullList` so the sweep doesn't re-fire on it); plus a bulk variant.
   In-flight status on `/missing` rows closes the loop. → Step 7

10. **Phantom-series UX disambiguation.** `/library/tidy`'s phantom
    section conflates true transition phantoms (had files, lost them)
    with newly-added-not-yet-fetched series (added via pull list,
    awaiting first download). Filter to transition phantoms, or split
    into two sub-sections. → Step 6b

11. **Dashboard polish.** Three small fixes: (a) the 8 stat tiles crowd
    the row — "NEEDS ATTENTION" wraps while peers stay single-line; (b)
    the "N issues missing across M series" sub-callout reads as a
    discrepancy against the SERIES tile (different denominators); (c)
    semantic colour coding on attention-state tiles is partial. → Step 1

---

## Workflow rules codified during A.9

Cross-cutting build, verification, and deploy rules that surfaced
mid-phase. Recorded here so they bind every remaining step rather than
being relearned per step.

- **Redeploy with `docker-compose up -d --force-recreate`.** Plain
  `up -d` no-op'd on the 6a deploy — it reported the container "Running"
  and left it on the stale pre-rebuild image. `--force-recreate` always
  recreates the container against the freshly built image. Use it as the
  standard redeploy command.

- **`cargo clippy --workspace --all-targets` per step.** A plain `clippy`
  check skips test code; the CBR hot-fix surfaced 6 pre-existing warnings
  visible only under `--all-targets`. Run it as a per-step verification
  flag, mirror of the `sqlx prepare --all-targets` rule below.

- **`sqlx prepare --workspace -- --all-targets`.** Established in A.8
  Step 7 — regenerate the `.sqlx` offline query cache across all targets
  whenever SQL changes, so the `SQLX_OFFLINE=true` container build stays
  in sync. Codified here alongside the others for visibility.

- **Dockerfile crate-COPY rule.** When a step adds a new workspace crate,
  update the `Dockerfile` stage-2 `COPY` list in the same commit —
  otherwise the hermetic build can't see the crate's sources. (A.9 added
  `longbox-archive` in the CBR hot-fix; its COPY line shipped in
  `f57a21c`.)

- **Colima mount fragility.** `/Volumes/Comics` is bind-mounted into the
  Colima VM over virtiofs; the VM's view can go stale when the host-side
  mount is touched (unmount/remount, sleep, drive swap), and
  `docker-compose up` then fails with `mkdir /Volumes/Comics: file
  exists`. `colima restart` re-syncs virtiofs and recovers it. Keep the
  host mount stable through A.9 work. (From the A.8 closeout doc.)

- **Idempotency on new insertion paths.** When a repo has an existing
  idempotency on key K (e.g. `cv_id` via `add_or_get_from_cv`) and a
  new code path inserts via a different shape, evaluate whether the
  new path needs its own idempotency on a different key. The 6a
  bulk-convert hot-fix surfaced this: the shallow path needed
  `(sort_title, start_year)` idempotency the cv_id path didn't
  provide. Forward question for every new insertion path — "what
  idempotency does the existing path provide, and does this one need
  its own equivalent on different keys?"

- **Spot-check anomalies as samples.** When a small unexpected datum
  surfaces (owned=0 on N=2 of an expected larger population), default
  to expanding the sample before concluding. A 2-case anomaly is more
  often a sample of a hidden population than an isolated outlier —
  the 6a→6b cleanup→parser sequence is the canonical demonstration:
  what looked like 2 visible Pattern A misses was actually a 4,182-file
  parser gap, and only an expand-the-sample pass exposed the real
  scale before another partial fix went out.

- **Cross-check catalog state against disk/external state during
  reconciliation archaeology.** Single-source data has blind spots
  that surface as quantitative-criterion misses after deploy. The
  37-CV-linked-zero-owned diagnostic missing the 22-ghost /
  15-no-series-row breakdown is the canonical demonstration — the
  parser hot-fix's <5 target was right for the wrong population
  because the catalog state didn't reflect disk truth, and only
  walking the disk per-folder exposed that 22 of 23 folders were
  gone and 26 separate folders were stuck in the F6 dismiss-trap.

- **Every CV-keyed affordance gates on `cv_id` presence.** Shallow
  series (`cv_id = NULL`) are first-class catalog citizens since 6a.
  CV-only UI that doesn't conditionally render will 400 the backend
  or render misleading copy for shallow rows. `SeriesHeader` and
  `IssueRow` are the right examples (everything CV-flavored sits
  under `{#if cv_id}` / `{#if cvUrl}`); the series-detail `Refresh`
  button was the wrong one. Forward question for every new CV
  affordance — "what happens when the series has no `cv_id`?"

---

## Deferred items

Not scheduled into a step — tracked here so they have a canonical home
rather than accreting in commit messages.

- **CBR/CBZ duplicate conflict.** (F5 from commit `f57a21c`.) A
  pre-existing `.cbr` and a later Phase B `.cbz` re-download of the same
  issue both survive: Phase B's conflict check tests only the `.cbz`
  target path, so the `.cbr` is not seen and the two coexist as
  duplicate files for one issue.

- **`needs_review`-as-resolved edge.** `detect_discovered_folders`
  treats any file with an `issue_id` — including low-confidence
  `needs_review` matches — as "resolved", so a real untracked folder
  whose only matched file is a weak match is wrongly excluded. Tighten
  the predicate to require `status='owned'`.

- **`pull_succeeded` webhook event.** Deferred from A.8 Step 10 — it
  needs Phase B's submitted→grabbed transition in `longbox-postprocess`.
  The future emit point is documented in code.

- **`new_solicitations` webhook event.** Deferred from A.8 Step 10 — it
  needs a ComicVine-polling delta detector. Future emit point documented
  in code.

- **Catch-all parser match-but-poison signal.** The parser hot-fix
  showed that pattern 4 (the catch-all `Series_NNN or Series NNN`)
  silently absorbed `Series (YYYY) NNN.ext` filenames with a
  corrupted `series_title` that baked the year in. Downstream
  consumers couldn't tell the difference between "parsed cleanly"
  and "parsed but the title is structurally wrong." Two candidate
  design moves: grow `ParsedFilename` a confidence signal (e.g. a
  `low_confidence: bool` set when the catch-all claims a filename
  whose series_title contains a parenthesized year-like substring,
  letting the scanner downgrade or skip), OR teach the catch-all to
  refuse-to-match when its candidate `series_title` ends in
  `(YYYY)`. Additional data point from the F6 hot-fix archaeology:
  `I Hate Fairyland (2022) (2022)` (duplicated `(YYYY)` in the
  folder name) is another shape pattern 15 absorbs but bakes one of
  the years into `series_title` — same poison class. Out of scope
  for the F6 hot-fix; warrants its own design surface because the
  choice between "annotate" and "refuse" affects every future
  catch-all-style pattern.

- **vN-as-issue-number collision (Bug 1b id=8 semantic).** Pattern
  id=8 (`Series vN - Subtitle (YYYY) ...`, priority 6) maps the
  volume number `N` to the issue number. Pragmatic for TPB-only
  series (Fear Agent, Promethea) where there's no underlying
  per-issue catalog. Hybrid series with both single-issue `#1` and
  TPB `v01` would collide on issue number 1 — same pattern would
  match BOTH the single issue and the TPB volume to "issue 1",
  failing the (series_id, number) unique constraint or attaching
  to the wrong row. Not blocking; flagging the semantic.

- **Bug 2 migration didn't auto-apply on container startup.**
  `20260526100000_dedup_series_across_null_year.sql` is embedded in
  the binary (confirmed via `strings`) and applies cleanly via
  `sqlx migrate run` CLI against the same DB, but did NOT auto-run
  on container startup after a `--no-cache` rebuild. Workaround:
  applied via CLI + DB swap. Root cause unknown — could be a sqlx
  0.7 `migrate!` macro quirk, a Docker build-context artifact, or
  something with the migration file content the macro silently
  skips. Tracked as a deferred investigation; if it recurs, it
  becomes a workflow rule ("apply migrations via CLI as belt-and-
  suspenders after each hot-fix deploy" or similar).
