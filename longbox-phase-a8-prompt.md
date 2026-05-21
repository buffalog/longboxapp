# LongBox — Phase A.8 brief

## Overview

**Frame: Pull list + solicitations + release calendar + auto-download.**

Phase A.8 brings LongBox to functional parity with Mylar's headline feature: subscribe to series, receive new issues automatically as they ship, browse upcoming releases industry-wide. This is the feature most directly affecting daily LongBox use for an active reader — closing the loop between "I know this series ships monthly" and "the new issue is in my library when I open the app."

Naming: **"Pull list"** as user-facing terminology, with the local-comic-shop-customer semantics (you maintain the list, the system pulls new issues as they ship). **"Solicitations"** as the issue status for cover_date ≥ today (the industry-native term used in Diamond's Previews catalog and across the comics press).

Phase A.8 is **not**:
- Theme support (light/dark/system) — pushed to Phase A.9
- Change CV Mapping — deferred to its own future brief
- Library Tidy / Normalize Layout — Phase C+ candidate
- Mylar-parity quality preferences (preferred groups, scanner filtering, RSS-based search) — A.8+ deferral queue
- Discord/Telegram/Signal first-class notifications — generic webhook ships in v1; service-specific formatting follows in A.8+

## Locked design decisions

- **Kickoff discipline applies per step.** Code surfaces 5-10 numbered implementation questions before writing code for each step, gets explicit approval, then implements. Same pattern as Phase B's per-step kickoffs.
- **Phase B is the receiving end.** Auto-pulled NZBs land in the watch folder; Phase B's existing pipeline catches them, catalogs them. The download bridge built in Phase B is the catch surface for A.8's auto-pull workflow. No re-implementation of post-processing logic.
- **No torrenting.** Newznab + Usenet only. Legal exposure non-negotiable; documented as a permanent scope exclusion.
- **Test coverage scaled to surface.** Backend integration tests for the pull engine, indexer client, downloader client. Frontend tests for new UI surfaces. End-to-end smoke against a real Newznab indexer + SAB instance before A.8 closes (analog to Phase B's manual smoke).
- **Each step = one commit.** Bundle fixes within a step are allowed; cross-step bundles are not.

## Locked architecture

### Naming, vocabulary, and predicates

- **User-facing:** "Pull list", "Solicitations", "Release calendar", "Pulled" / "Not pulled"
- **Schema/code:** technical names where they read better (`is_on_pull_list`, `cover_date >= today` predicate)
- **Solicitations predicate:** `cover_date >= CURRENT_DATE`. No `is_solicited` column; computed dynamically. Avoids periodic-update problem (solicited issues age into "missing" naturally as cover_date passes).
- **Status rendering:** `/series/:id` issue rows render "Solicited" status when `cover_date >= today` AND no owned file exists. Fixes the current asymmetry where `/missing` distinguishes solicited from missing but `/series/:id` treats both as Missing.

### Settings pattern

- New tables for DB-stored editable indexer + downloader config:
  - `downloader_config` — single-row, type ∈ {sab, nzbget}, base_url, api_key, category, enabled
  - `indexer_configs` — multi-row, name, base_url, api_key, enabled, priority, maxage_days
  - `webhook_configs` — multi-row, name, url, event_mask (bitset of notification events), enabled
- Settings UI gets three new sections following Publisher filters pattern (form + table + edit/delete per row)
- Existing env-driven Configuration block stays as-is (no migration of existing fields)

### Pull list schema

- `pull_list` table — series_id, added_at, start_issue (optional, default null = pull from first solicited), paused (boolean), last_pull_attempt_at, last_successful_pull_at, failure_count
- `pull_attempts` table — id, series_id, issue_id, attempted_at, indexer_id, release_id, status ∈ {pending, submitted, grabbed, failed, mismatched}, error_message, retry_count
- Per-series pull state lives on the `pull_list` table; per-attempt history lives in `pull_attempts` for audit + retry exclusion tracking

### Failure surfacing

- **Rename `/files/pending-intervention` → `/needs-attention`.** Phase B copy ("Files Phase B couldn't process automatically") replaced with phase-agnostic language.
- **Single broadened attention queue** with categorized sections:
  - **Conflict** (file landed, target exists — Phase B's existing category)
  - **Submission failed** (NZB couldn't reach downloader)
  - **Grab failed** (downloader couldn't finish; par2/incomplete/expired)
  - **No match** (indexer query returned zero results for a solicited issue)
  - **Mismatched grab** (file arrived but didn't match expected series/issue)
- Each category surfaces retry/manual-action options where applicable

### Retry strategy per failure mode

| Failure | Strategy |
|---|---|
| NZB submission failed | Exponential backoff: 1m / 5m / 30m, then attention queue. Mostly transient (indexer or downloader down). |
| NZB grab failed | Permanent for that release_id. Retry = re-query indexer, exclude already-tried release_ids. Up to 3 different NZBs before giving up. |
| No match found | Scheduled retry on next sweep cycle (no immediate retry). |
| Mismatched grab | Manual intervention only in v1. Post-processing validation (auto-detect wrong-grab) deferred to A.8+. |

### Notifications

- **Generic webhook** primary mechanism. POST JSON to user-configured URLs.
- **Slack-first formatting:** when URL host is `hooks.slack.com`, payload uses Slack block kit formatting. All other URLs receive plain JSON.
- **Events:** pull succeeded, pull failed (final, after retry exhaustion), new solicitations for pulled series, pull engine error.
- **Per-webhook event mask:** user picks which events trigger each webhook.
- **Discord/Telegram/Signal:** deferred to A.8+. Signal especially because no native webhook exists.

### Indexer + downloader protocol

- **Indexers:** Newznab protocol (covers Prowlarr aggregation + direct indexers like NZBgeek, DrunkenSlug, etc.)
- **Downloaders:** SABnzbd OR NZBGet (both have stable HTTP APIs); single downloader configured at a time
- **Auth:** API keys per indexer + per downloader, stored in DB

### NZB matching strategy

- **Query format:** Try two variations per indexer if first returns zero hits:
  1. `{series} {issue:03d}` (e.g., "Wolverine 005") — three-digit zero-pad
  2. `{series} {issue}` (e.g., "Wolverine 5") — no padding
  - Solves Mylar's known zero-padding mismatch issue (mylar3 #689) without complex per-indexer logic
- **Year disambiguation:** Include `({year})` in query when series has multiple known CV volumes; skip year otherwise to avoid noise
- **Category:** Newznab `cat=7030` (comics)
- **`maxage`:** configurable per indexer, default 1500 days (~4 years)
- **Provider iteration:** ordered list by priority, first indexer with results wins (matches Mylar behavior)
- **Defensive parsing:** all indexer responses parsed with Rust's `Option<T>` discipline — missing fields handled gracefully, never panic
- **Result selection within an indexer:** prefer cbz (sort-key), take highest `grabs` count, tie-break by recency

### Format acceptance

- **Both cbz AND cbr accepted.** cbz preferred only as sort-key when multiple results match same issue. cbr files are 100% catalogable — LongBox doesn't extract archive contents, just reads ComicInfo.xml from inside (universal across formats).

### Pull engine schedule

- **Scheduled** daily at configurable time (default 5am local). Cron-style; no event-driven complexity.
- **Manual trigger:** "Check now" button on pull list page forces immediate sweep.
- **Per-series last_check_at** tracking prevents re-querying solicitations that haven't progressed.

### Caching strategy

- **Two separate caches with different lifecycles:**
  - **Release calendar cache (user-facing):** short-TTL (default 1 hour), refresh on user query when stale. For the manual release calendar view.
  - **Pull engine cache (system-facing):** scheduled refresh (daily, aligned with pull engine sweep). For the auto-pull workflow.
- Both stored in DB tables — `cv_release_cache` keyed by date range + publisher

### Catalog integration with Phase B

- When auto-pulled file lands → Phase B catches → catalogs with `match_method = 'pull_list'` (distinct from `'phase_b'`)
- Activity feed differentiates auto-pulls from manual SAB drops visually
- Catalog audit becomes possible ("which series have been auto-pulled successfully, which manually dropped")

### Nav + Dashboard

**Nav restructure:**

```
Dashboard | Library ▾ | Releases ▾ | Add | Settings
              ├─ Series              ├─ Calendar
              ├─ Files               ├─ Pull list
              ├─ Missing             └─ Releases of note
              └─ Scans
```

5 top-level items (was 7). Library groups catalog-state surfaces; Releases groups discovery surfaces.

**Dashboard widgets (added):**

1. **"This week's pulls"** — solicited issues from pulled series with cover_date in current ship-week (Wed–Tue). Cover thumbnails + series + issue + cover_date.
2. **"Releases of note"** — series-name-substring match against owned series, surfaces this-week solicitations not on pull list. Discovery affordance.

**Dashboard counter tiles:** add ONE — "Pull list" (count of subscribed series). "Upcoming pulls" and "Pull failures" live in widgets/pages, not tiles.

**"Pick of the Week" from iFanboy/similar:** deferred to A.8+ pending demand. External aggregation surface adds complexity not justified for v1.

### Release calendar UI

- **Tabular** (matches /missing pattern): date | cover | series | issue | publisher
- **Filters:** date range (default this-week Wed–Tue), publisher (all / multi-select), pull list (all / pulled / not-pulled)
- **Manual "Refresh CV" button** invalidates cache, re-queries CV across all publishers for current range
- **Per-row action:** "Add to pull list" button on series-not-already-pulled rows

### Series detail "Add to pull list"

- Toggle/icon-button in series header near Refresh button
- State indicator: filled icon + "On pull list" label when subscribed
- Optional secondary action: "Pause pulls" (stops auto-pulls without removing from list)

## Phase kickoff resolutions (2026-05-19)

Cross-cutting decisions settled at phase kickoff, before Step 1:

- **Crate structure.** Three new workspace crates, matching the
  one-crate-per-external-integration precedent set by
  `longbox-comicvine`: `longbox-newznab` (Step 1 indexer client),
  `longbox-downloader` (Step 2 SAB/NZBGet client), `longbox-pull`
  (Step 6 pull engine). `longbox-core` stays pure-logic.
- **Pull engine model.** Lives in `longbox-pull`, started from
  `longbox-web` at boot the same way Phase B's watcher is
  (`longbox_postprocess::start` precedent). In-process scheduling via
  a tokio task that sleeps until the configured daily time — no
  external cron, no scheduler dependency. Manual "Check now" triggers
  the same sweep function via a shared handle.
- **Pull-grab detection (how Phase B stamps `match_method='pull_list'`).**
  `pull_attempts`-table lookup. The pull engine writes a
  `pull_attempts` row (series_id, issue_id, status='submitted') on
  NZB submission. Phase B's processor, after the matcher resolves a
  file to (series_id, issue_id) and before the catalog upsert,
  queries for a pending `pull_attempt` matching that pair — found →
  `match_method='pull_list'` + transition the attempt(s) to
  'grabbed'; not found → `match_method='phase_b'`. Downloader-agnostic.
  Adds one `longbox-db` query to `longbox-postprocess`'s processor.
  **Race handling (Step 6):** if 2+ pending `pull_attempts` rows
  exist for the same (series_id, issue_id), transition *any/all* of
  them to 'grabbed' when Phase B finds the match.
- **`MatchMethod` enum.** Gains a `pull_list` arm in `longbox-core`,
  with a migration extending the `match_method` value set — lands in
  Step 3 alongside the schema work (same shape as Phase B's `phase_b`
  addition).

## Steps

The following step breakdown is a starting point; per-step kickoffs may surface scope adjustments. Each step is a single commit.

### Step 1: Newznab client
- Rust crate or module: `longbox-newznab` (or in `longbox-core`)
- Newznab API client: search query construction, response parsing, error handling
- Provider iteration logic (priority order, first-with-results wins)
- Defensive `Option<T>` parsing throughout
- Unit tests: query construction, response parsing (including malformed responses), provider iteration

### Step 2: Usenet downloader client
- Rust module: `longbox-downloader` (or in `longbox-core`)
- SABnzbd API client (auth, submit NZB, query status)
- NZBGet API client (same surface)
- Common trait abstraction: `Downloader` with submit/status methods, implemented by both
- Unit tests: API call construction, response parsing, error handling

### Step 3: Pull list schema + repo methods
- DB migration: `pull_list`, `pull_attempts`, `downloader_config`, `indexer_configs`, `webhook_configs`, `cv_release_cache` tables
- Repo methods in `longbox-db`: CRUD for each table
- Unit tests against in-memory SQLite

### Step 4: Solicitations rendering
- `/series/:id` issue table renders "Solicited" status when `cover_date >= today` AND no owned file
- Predicate-only — no schema change to issues table
- Frontend test: solicited vs missing distinction on series detail page

### Step 5: Settings UI for indexers + downloader + webhooks
- Three new sections in `/settings` matching Publisher filters pattern
- Add/edit/delete forms for each
- Test connection buttons for indexer + downloader (ping API, verify auth)
- Frontend tests for CRUD interactions

### Step 6: Pull engine orchestration
- Background job (scheduled cron + manual trigger)
- For each pulled series: query indexers for solicited issues with cover_date ≤ today
- On match: submit NZB to downloader, write `pull_attempts` row, transition state
- On failure: apply retry strategy per failure mode, update attempt count
- Integration tests with mock indexer + mock downloader

### Step 7: Pull list management UI
- `/releases/pull-list` route
- List view: series name + status (active/paused) + last pull date + failure count + "Pause" / "Resume" / "Remove" actions
- Series detail page: "Add to pull list" toggle in header

### Step 8: Release calendar UI
- `/releases/calendar` route
- Tabular layout with filters (date range, publisher, pull list status)
- Cache-aware: short-TTL cache, "Refresh CV" button forces invalidation
- Per-row "Add to pull list" action

### Step 9: Releases of note widget
- Dashboard widget: the current ship-week release calendar (`cv_release_cache`, the Step 8 data source) filtered to volumes whose name matches a series the user *owns* and that is *not* on the pull list — a discovery affordance.
- Match runs route-side, not in a repo method: the calendar payload is cached CV JSON in `cv_release_cache.payload_json`, not DB rows, so there is no table to `LIKE`-join against. `GET /api/releases/of-note` reuses `load_calendar` + `series_repo::find_all_with_counts` (`owned_count > 0`) + `pull_list_repo::list_all`, and matches with `longbox-core::normalize_title`.
- **Match semantics + known v1 tradeoff:** a release is "of note" when an owned series' `sort_title` is a *substring* of the release's normalized `volume_name`. This can false-positive on short owned titles (`hawk` ⊂ `hawkeye`). Accepted for v1 — the widget is a low-stakes discovery surface, not a critical path. Deferred refinement: word/token-boundary matching to kill the short-title false positives (A.8+ / B+ queue).
- Frontend test: widget renders correctly with mock data.

### Step 9b: This week's pulls widget + Pull-list counter tile
Inserted after Step 9 (the remaining items from the brief's "Dashboard widgets (added)" / "Dashboard counter tiles" sections that Step 9's narrow scope left unstepped — kept separate from Step 12's nav restructure, which is structural not widget work):
- **"This week's pulls" widget** — the current ship-week's release calendar (`cv_release_cache`, the Step 8 data source) filtered to issues whose volume is on the pull list. Per-issue, dated by `store_date`. **Supersession note:** the brief's original framing for this widget — "solicited issues … `cover_date` … catalog `issues`" — predates Step 8's decision to model on-sale dates with `store_date`. The catalog `issues` table carries only `cover_date` (the cover-printed month, ~2 months ahead of on-sale), so it cannot answer "shipping this week" accurately. Step 9b's data source is therefore the calendar firehose filtered to pull-list `cv_id`s — the inverse membership filter of Step 9's of-note widget — **not** catalog issues.
- **"Pull list" counter tile** — count of subscribed series, added to the dashboard tile row.

### Step 10: Notifications + webhook dispatch
- `longbox-webhooks` crate — a pure delivery client: `deliver(url, event)` POSTs the event, with Slack block-kit formatting for `hooks.slack.com` hosts and plain JSON otherwise, and count-based in-memory retry (3 attempts, no persistent retry queue).
- Dispatch wiring in `longbox-pull`: on a pull-engine event, fan out to every enabled webhook subscribed to it (`webhook_config_repo::list_subscribed`), spawned fire-and-forget so webhook latency never blocks a sweep.
- `POST /api/webhooks/:id/test` + a per-webhook "Test" button in the Settings webhook UI.
- Per-webhook event-mask filtering (the `event_mask` bitset + `EVENT_*` constants already exist).
- Integration test against a local HTTP mock server.

**v1 event scope — `pull_failed` + `pull_engine_error` only.** Of the four `EVENT_*` bits, only these two have a clean, dedup-safe emit point in the pull engine; both are wired in Step 10. The other two are **deferred by architecture, not oversight** — their bits and UI checkboxes remain (forward-compat; the "Test" button still exercises any webhook):
  - **`pull_succeeded`** — its correct emit point is Phase B's `submitted → grabbed` `pull_attempt` transition in `longbox-postprocess`, where a download is actually finalized and catalogued. It cannot fire from the pull sweep: the sweep only *observes* `DownloadStatus::Completed` while the attempt is still `submitted`, and can re-observe it on a later sweep (e.g. if Phase B is disabled) — no dedup. Wiring it belongs to a `longbox-postprocess` event hook.
  - **`new_solicitations`** — requires a ComicVine-polling delta detector for tracked-series new-issue detection; no such subsystem exists. Per the locked architecture decision ("no CV refresh during pull sweeps; new-issue discovery is the release calendar's job, via `cv_release_cache`"), new-issue detection belongs to calendar-cache work, not the pull engine. The event has no emitter until that subsystem exists.

### Step 11: Failure surface rebrand
- Rename `/files/pending-intervention` → `/needs-attention`; a two-section page — pull failures + Phase B manual intervention.
- Failure categories — **v1 ships five, the ones with real data:** Phase B's three (Conflict, ComicInfo write failed, Move failed) + pull's two (Submission failed, Grab failed — split from `pull_attempts.status = 'failed'` by whether `release_id` is set: a submit that never landed a release vs. a submitted download that then failed).
- **Deferred categories — by data availability, not oversight:**
  - **No match** — the sweep counts `no_match` but writes no row; `pull_attempts.status` has no `no_match` value. Surfacing it needs sweep-side persistence: a `status` (or table) addition plus the sweep writing a row when an indexer search returns nothing. Out of Step 11's rebrand scope.
  - **Mismatched grab** — `pull_attempts.status` defines `mismatched`, but nothing sets it. It needs post-grab content verification (does the landed file match the issue it was pulled for?) — the A.8+ "Post-processing validation / auto-detect mismatched grabs" detector, which does not exist. The category surfaces once that detector is built.
- Retry — **pull-failure rows only.** A "Retry" un-parks the issue (clears its `failed` `pull_attempts` rows) and nudges an immediate sweep. **Phase B intervention rows get no retry button:** the post-process watcher re-triggers naturally — when the user resolves the underlying conflict on disk, the next filesystem event re-runs processing for that file. A manual re-process entry point would land in `longbox-postprocess` (alongside `start`) if dogfooding shows the fs-event path is insufficient; v1 does not add it.
- Cross-link — the pull-list page gets an inbound indicator linking to `/needs-attention`; `/needs-attention` pull-failure rows link out to `/series/:id`. **The release-calendar cross-link from the original brief is dropped** — the calendar is forward-looking (upcoming releases) and failures are backward-looking; there is no natural composition.

### Step 12: Nav restructure
- Top nav: Dashboard | Library ▾ | Releases ▾ | Add | Settings
- Dropdown menus or tabbed sub-pages (kickoff decision)
- Frontend tests: nav navigation works correctly

### Step 13: End-to-end manual smoke
- Real Newznab indexer + real SAB/NZBGet downloader + real watch folder
- Pull list: add series, wait for sweep, verify NZB submitted + file landed + Phase B catalogued with match_method='pull_list'
- Failure paths: kill SAB mid-grab, verify retry; bad API key, verify attention surface; etc.
- Webhook delivery: verify Slack receives formatted message
- Closeout doc: `longbox-phase-a8-closeout.md` if anything unexpected surfaces

## Done definition

Phase A.8 is done when:

1. All 13 steps committed with locked decisions honored
2. Workspace tests green after each step
3. End-to-end manual smoke validates the full pull-to-catalog chain against real services
4. Catalog math invariant verified post-smoke (auto-pulled files appear with `match_method='pull_list'`, counters reconcile)
5. Webhook delivery confirmed against Slack (or other target)
6. Brief + closeout doc tracked in repo

## Out of scope (deferred)

- **Discord / Telegram first-class formatting** → A.8+
- **Signal notifications** → A.8+ pending clean integration path
- **Mylar-parity quality filtering** (preferred groups, scanner filtering, RSS-based search) → A.8+ queue
- **Post-processing validation** (auto-detect mismatched grabs) → A.8+
- **"Pick of the Week" from iFanboy** → A.8+ pending demand
- **Creator-based release recommendations** → A.8+ pending CV creator metadata in catalog
- **Multi-downloader support** (SAB AND NZBGet simultaneously) → A.8+ if real demand surfaces
- **Indexer API rate-limit handling** → A.8+ if real-world hits show this is needed (Mylar's "manual throttle" experience)

---

## Note on phase ordering

Phase A.8 is independent of Phase A.7 (UI/UX polish) at the technical level — they touch different surfaces. A.7 (series detail, scrubber, sticky nav, discoverability + keyboard, toast infrastructure) completed 2026-05-19, before A.8 kickoff.

After A.8 lands, the roadmap looks like:
- **A.9 — Theme support** (light/dark/system; deferred from the original A.8 slot)
- **A.10 — Change CV Mapping** (corrective action for matcher choosing wrong volume)
- **B+ queue items** (PollWatcher fallback for virtiofs/network filesystem environments)
- **C — Library Tidy / Normalize Layout** (using Phase B's building blocks to bring existing library files into convention compliance)
- **A.8+ queue items** (Mylar-parity quality filtering, post-processing validation, Discord/Telegram notifications)
