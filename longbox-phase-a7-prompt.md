# LongBox — Phase A.7 brief

## Overview

**Frame: UI/UX polish + per-issue interaction.**

Phase A.7 follows Phase B's "download bridge" landing with a coherent polish phase organized around per-issue interaction (the meatiest task — series detail UI upgrades) plus smaller items that improve daily-use friction (alphabetical scrubber, sticky nav, discoverability + keyboard bundle, toast infrastructure).

Phase A.7 is **not**:
- Theme support (light/dark/system) — deferred to its own dedicated Phase A.8 brief due to scope (touches every component)
- Change CV Mapping — deferred to future brief due to scope (new backend endpoint + re-match logic + destructive-action UX is its own feature, not polish)
- Library Tidy / Normalize Layout — deferred to Phase C+ (Phase B's building blocks are accumulated; this surfaces them via user-facing trigger)
- Per-issue activity feed dedup, reprint filter publisher expansions, smart completionist filters — lower-priority deferrals from earlier phases; stay in notes until a real need surfaces

## Locked design decisions

- **Kickoff discipline applies per task.** Code surfaces 5-10 numbered implementation questions before writing code for each task, gets explicit approval, then implements. Same pattern as Phase B's per-step kickoffs.
- **Test coverage scaled to surface.** Frontend-only tasks: integration tests where the surface is non-trivial (Task 1, Task 4, Task 5). CSS-only tasks: visual verification + workspace tests still green (Task 2, Task 3).
- **Each task = one commit.** Bundle fixes within a task are allowed; cross-task bundles are not.
- **Brief updates land in same commit as code when design changes** (per Phase B precedent).

## Preflight item (before Task 1 starts)

### P1. Completion% consistency reconciliation

Outstanding Task 5 follow-up. Half-verified during Phase B drive: `/missing` uses STRICT semantics (includes solicited issues with the "Solicited" label). `/series` completion% sort formula remains unverified from sort position alone.

**Action:** Code reads the `/series` completion% calculation in `longbox-web` (likely in the series list endpoint or computed in the frontend). Reports back:
- If `/series` also uses strict semantics → consistency confirmed, no action needed
- If `/series` uses available-only → inconsistency surfaced, becomes a Task 0 bug fix before Task 1 starts

Five-minute code read. No code change unless inconsistency found.

> **Resolved at A.7 kickoff:** consistent. Both surfaces use the same
> "no owned+present file exists for the issue" predicate
> (`series_repo::find_all_with_counts` SQL + `routes/missing.rs`).
> `total_count` includes solicited issues; completion% = owned/total
> reflects them as not-yet-owned, matching `/missing`'s strict
> semantics. No Task 0 needed.

> **Previously P2 (`?sort=` URL sync on `/series`):** struck from this
> brief. Already shipped in commit `ac4b6b7` during the Phase B-era
> A.6 fix-up bundle, before A.7 kickoff. Task 2 no longer carries it
> as a bundled deliverable.

---

## Tasks

### Task 1: Series detail UI — first slice

**Scope:** per-issue inline row expand showing synopsis + larger cover + CV link, plus a deliberate series-level CV link in the header. The series-level link is a design-intent affordance — an always-present, controllable surface that doesn't depend on whatever links CV happens to embed inside description HTML.

**Locked decisions:**

- **Row-expand trigger:** click on the title cell specifically. Not whole-row (file path link / status pill / Copy issue ID button have their own click semantics). Not chevron icon (clutters).
- **Expand animation:** `max-height` transition over 200-300ms. Standard accordion behavior.
- **Multiple rows expanded:** independent. No auto-collapse-others when a new row expands.
- **Content layout when expanded:**
  - Cover on left, larger thumbnail (~80×120; current thumbnail is ~40×60)
  - Synopsis on right, wraps naturally
  - CV link below synopsis (or aligned bottom-right of cover column)
- **HTML rendering for synopsis:**
  - Sanitized HTML via Svelte's `{@html}` directive
  - DOMPurify (or equivalent) for sanitization to prevent XSS from CV-side content
  - Sanitize **lazily on row expand**, not on page load — only the expanding row pays the cost; page-load isn't gated by sanitizing all synopses upfront
- **Empty-state — issue with no synopsis** (missing/unsolicited):
  - Show the expand affordance regardless
  - Display "No synopsis available" message + the larger cover (which IS usually available even for solicited issues)
  - Consistency in interaction (every row expands) beats inconsistent affordances
- **Series-level CV link:**
  - **Deliberate, always-present header affordance.** Description HTML from CV may or may not include a self-link in any given series; that's not a design surface we control. The header link is.
  - Placement: series header, near the Refresh button
  - Styling: text link with external-link arrow — `View on ComicVine ↗`
- **Per-issue CV link:** same styling pattern as series-level — `View on ComicVine ↗`
- **Keyboard interaction:**
  - `Enter` on focused title cell → expand
  - `Esc` → collapse currently-focused expanded row
  - `j/k` → navigate between rows (matches /files triage pattern; no conflict — different page, different context)
  - No `a/m/i` shortcuts here (those are triage-specific; series detail is a read-only catalog view)

**Out of scope for this task:** notes, tags, reading state, Metron infrastructure, "Change CV Mapping" corrective action, per-issue file path / size / mtime display, variants. All deferred to future briefs.

### Task 2: Alphabetical scrubber

**Scope:** narrow vertical strip on the right edge with letter labels that jump-scroll to the first entry starting with that letter. Applies to `/series` and `/missing?sort=series`.

**Locked decisions:**

- **Toggle approach: contextual auto.** Scrubber appears only when list exceeds ~1.5 viewports of scrollable content. Auto-hides otherwise. Self-justifying, no extra UI to manage, no manual user setting needed.
- **Visual treatment:**
  - Narrow strip (~16px wide) on right edge of content area
  - A–Z letters + single `#` entry after Z (iOS contacts pattern for numeric/symbol prefix entries)
  - Letters in small text (10-12px), vertically distributed, ~24px tall per letter row
  - Dimmed/muted color when no entries match that letter; primary color when entries exist
- **Click behavior:** smooth-scroll via `element.scrollIntoView({ behavior: 'smooth' })`. Sudden jumps are jarring; smooth-scroll preserves spatial awareness.
- **Mobile touch-and-drag preview (iOS contacts pattern):** **defer to v2.** v1 ships with click-to-jump that works on touch devices natively. Touch-and-drag is its own future enhancement if mobile use surfaces real friction.
- **Positioning:** `position: fixed; right: 16px; top: 50%; transform: translateY(-50%);` — vertically centered on viewport. Standard contacts-app pattern.
- **Applicability:**
  - `/series` — always (subject to contextual auto threshold)
  - `/missing?sort=series` — only when sorted by series (group headers double as anchors)
  - `/missing` cover-date sort — scrubber hidden entirely (order isn't alphabetical)
- **URL fragment on click:** **defer to v2.** Click-to-smooth-scroll only in v1. URL fragment for shareable jumps is a future enhancement.
- **`#` bucket behavior:** clicking `#` lands user at the first numeric-prefix entry (e.g., "1776" before "300" in alphanumeric order). User scrolls within bucket for specific entry. Acceptable friction for typical comics catalog numeric-prefix populations.

### Task 3: Sticky top navigation

**Scope:** Make the top navigation stick to the viewport so it remains accessible during scrolling. Daily-use polish.

**Locked decisions:**

- **CSS approach:** `position: sticky; top: 0;` on the nav wrapper in `+layout.svelte` (or wherever the nav lives).
- **Background treatment on scroll:** translucent with backdrop blur — `background: rgba(var(--bg-rgb), 0.85); backdrop-filter: blur(8px);`. Modern polish; scales to dark mode (Phase A.8) without redesign. Solid background fallback for browsers without backdrop-filter support.
- **Shadow on scroll:** subtle box-shadow appears when `scrollY > 0` — `box-shadow: 0 1px 2px rgba(0,0,0,0.05);`. No shadow at top-of-page (nav is naturally on its own background). Bind to scroll state via Svelte reactive statement or vanilla listener.
- **Mobile behavior:** always sticky. No auto-hide-on-scroll-down pattern. LongBox isn't mobile-first; nav doesn't eat much vertical space (~48px); auto-hide adds animation complexity that doesn't pay off for catalog management.
- **z-index:** 50. Code verifies at implementation time against the existing modal z-index stack (modals should be 100+); adjust if conflict.

### Task 4: Discoverability + keyboard bundle

**Scope:** three small UX nudges sharing the "discoverability + keyboard" character.

#### Sub-item A: Multi-select discoverability hint on flat /files view

**Scope:** point flat-view users to folder view's bulk-match workflow.

**Locked decisions:**

- **Placement:** inline with the view toggle (Flat / By folder buttons). Small text adjacent to the buttons.
- **Copy:** "Tip: Try **By folder** for bulk match" (or similar; final wording at implementation discretion as long as it's concise and points at the action).
- **Visibility:** always visible when flat view is active and there are >0 unmatched files. No dismissibility logic (no banner-X-state to persist). No conditional escalation logic (no "show after N single-file matches").

#### Sub-item B: `s` shortcut on folder cards

**Scope:** bring keyboard navigation to the folder card view (`/files?view=folder`), mirroring Task 6 (A.6)'s Needs Review row keyboard workflow.

**Locked decisions:**

- **Keyboard map:**
  - `j/k` → navigate between folder cards (focus next/previous)
  - `s` → trigger Search ComicVine modal on focused folder card
  - No `a/m/i` shortcuts (triage-specific; folder cards have one primary action)
- **Focus indicator:** visible focus ring on the focused folder card (CSS outline/ring matching existing focus styles).
- **Help modal (`?` overlay):** add "Folder cards" section alongside existing Needs Review shortcuts.
- **Help line at top of `/files?view=folder`:** mirrors existing Needs Review pattern — `j/k navigate · s search comicvine · ? for help`.

#### Sub-item C: Scan card semantic clarity

**Scope:** rename or restructure scan card UI columns so the math is obvious in user-facing context. "Seen 0" was specifically confusing during the Phase B smoke pass.

**Locked principle:** **UI labels should be semantically clear in user-facing context, not just DB column passthroughs.**

**Kickoff-time action:** Code reads `ScanCard.svelte` (or wherever the scan card UI lives), surfaces current column names that confuse, proposes user-facing renamings or restructurings, gets approval before changing. No predefined renaming locked at this point — depends on what Code finds.

### Task 5: Toast infrastructure

**Scope:** graduate the component-local "Copied!" indicator on `IssueRow.svelte` to a shared `Toast.svelte` + store. Build the infrastructure cleanly; v1 migrates one existing indicator.

**Locked decisions:**

- **V1 scope:** migrate "Copied!" only. No new call sites in this task. Future tasks adopt toast as needs surface organically.
- **Position:** bottom-right corner. Doesn't obscure nav, not in primary visual path, standard pattern.
- **Size:** ~280px wide, single line typical, multi-line allowed. Icon + message.
- **Animation:** slide-in from right (200ms) + fade-out (150ms) on dismiss.
- **Auto-dismiss duration:**
  - 3 seconds — success / info
  - 5 seconds — warning / error
- **Toast types: four total** — success, error, info, warning. Different colors, icons, default durations.
- **Stacking:** vertical stack, newest on top, max 3 visible. Older toasts auto-dismiss when limit is hit. Prevents toast spam while allowing batch-operation visibility.
- **Dismissibility:** both auto-dismiss AND X button. Click-anywhere-to-dismiss not implemented (toasts may contain action buttons / links).
- **Store API shape:**
  - Typed methods: `toast.success("Message")`, `toast.error("Message")`, `toast.info("Message")`, `toast.warning("Message")`
  - Advanced: `toast.show({ message, type, duration, action? })` for custom cases
- **Migration scope at kickoff:** Code does archaeology — greps for component-local notification patterns, surfaces them in the kickoff Q&A, but doesn't necessarily migrate all in this task. Task 5 v1 = build infrastructure + migrate "Copied!" + document the pattern. Future tasks adopt as natural use cases surface.

---

## Done definition

Phase A.7 is done when:

1. **Preflight resolved:** P1 (completion% consistency) verified — formula consistency confirmed OR Task 0 fix applied. (P2 was struck at A.7 kickoff; already shipped.)
2. **All five tasks committed** with locked decisions honored
3. **Workspace tests green** after each task
4. **Visual verification on per-task basis** — frontend changes verified in browser (live container) before each commit, OR skipped explicitly per task character (e.g., Task 3 sticky nav is pure CSS, can ship on visual eyeball alone)
5. **A.7 closeout doc** if any unforeseen patterns emerge worth documenting (analog to `longbox-phase-b-closeout.md`)

## Out of scope (deferred)

- **Theme support (light / dark / system)** → Phase A.8 brief
- **Change CV Mapping** → future brief; backend feature with re-match logic, not polish
- **Library Tidy / Normalize Layout** → Phase C+ candidate; Phase B's building blocks accumulated; needs user-facing trigger and orchestration
- **Per-issue notes / tags / reading state / Metron linking** → future series detail UI slices
- **Mobile touch-and-drag preview on scrubber** → scrubber v2
- **URL fragment on scrubber click** → scrubber v2
- **Activity feed per-issue dedup** → lower priority; revisit when batch imports surface as actual UX issue
- **Reprint filter publisher expansions** → lower priority
- **Smart completionist filters on /missing** → lower priority; revisit when missing list grows
- **Phase B+ items** (PollWatcher fix for virtiofs, etc.) → separate B+ queue, tracked in `longbox-phase-b-plus-queue.md`

---

## Note on phase ordering

Phase A.7 can run in parallel with Phase A.8 (theme support) if desired — they touch different surfaces. A.7 is per-issue interaction + polish; A.8 is whole-system theming. No technical dependency between them. Sequencing is a workload management decision, not a technical one.

After A.7 and A.8 land, the next natural phase candidates are (in rough order):
- **A.9 — Change CV Mapping** (the corrective action deferred from Task 1)
- **B+ queue items** (starting with PollWatcher fallback for production virtiofs deployments)
- **C — Library Tidy / Normalize Layout** (the future-phase candidate from the notes)
- **D — Indexer/downloader integration** (the originally-planned Phase C, the full evilhero/mylar feature parity)
