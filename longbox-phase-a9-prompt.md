# LongBox Phase A.9 — feature queue from the dogfood pass

Eleven improvements surfaced while dogfooding the A.8 release surfaces
and Library Tidy. This brief is the canonical A.9 plan; per the A.8
precedent it is amended as each step resolves.

**Convention:** each step gets a kickoff (numbered implementation
questions, each with a recommendation) → approval → a single
implementation commit → container rebuild + redeploy.

**Status (2026-05-22):** Step 4 in progress. Steps 1–3, 5, 7 queued;
Step 6 sequenced behind Step 4. The A.8 closeout smoke
(`longbox-phase-a8-closeout.md`, Step 13 of the A.8 brief) runs after
Step 6 — once Library Tidy reflects accurate disk truth.

---

## Step structure

A.9 is seven steps, grouped by surface, ordered by dependency and
impact.

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

### Step 6 — Library Tidy accuracy & UX · items 2, 10 (+ scope expansion)

Auto-tidy on folder removal (item 2) and phantom-series UX
disambiguation (item 10). Sequenced behind Step 4 (inherits its bulk
primitives) and behind a full scan (needs accurate Library Tidy data).

**Scope expansion:** Step 6 also covers bulk-converting `/library/tidy`
untracked folders → tracked series, for the ~563-folder backlog the CBR
hot-fix surfaced. Implementation approach is TBD at the Step 6 kickoff —
candidates: a background queue with a ComicVine throttle, a shallow add
without CV enrichment, or bulk-add with a throttle. Tradeoffs surface
there. Step 6 also refactors `/library/tidy`'s two hand-rolled bulk
sections onto Step 4's `BulkActionBar` + `createSelection`.

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
   setting, defaulted on. → Step 6

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
    into two sub-sections. → Step 6

11. **Dashboard polish.** Three small fixes: (a) the 8 stat tiles crowd
    the row — "NEEDS ATTENTION" wraps while peers stay single-line; (b)
    the "N issues missing across M series" sub-callout reads as a
    discrepancy against the SERIES tile (different denominators); (c)
    semantic colour coding on attention-state tiles is partial. → Step 1

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

- **`clippy --all-targets` workflow rule.** The CBR hot-fix surfaced 6
  pre-existing clippy warnings visible only under `--all-targets` (test
  code), meaning prior steps' clippy checks did not use it. Codify
  `cargo clippy --workspace --all-targets` as the standing rule, mirror
  of the `sqlx prepare --workspace -- --all-targets` rule codified in
  A.8 Step 7.
