# LongBox state-of-system snapshot

Date: 2026-05-31, late.
Audience: future-Claude / Jude picking up the LongBox thread.
Supersedes: the three prior 2026-05-31 / 2026-06-01 snapshots and the
A.9 handoff doc (all deleted this session as superseded).

## 1. What closed this session

The **Item A v2 arc is complete.** Forward-week release calendar
backed by Metron is live, the pull-list endpoint accepts Metron-only
rows, and the regression that was masking the per-row checkbox bug
is fixed and tested.

Commits since the last snapshot:

```
dcd3623 a9 Item A v2 piece 4 / Option C — live probe + close-out notes
e2b176f a9 calendar: regression tests for per-row checkbox isolation on Metron rows
9ceab4b a9 Item A v2 piece 4: Option C — resolve cv_id at subscription via Metron
```

The Option C probe doc at `docs/longbox-a9-option-c-probe.md` has the
specifics (live API probes, SQL verification of the metron_id
backfill, the catalog-cache-hit assertion, the 404 negative). Don't
re-read it unless you need to dive on Option C internals.

## 2. The checkbox bug episode — load-bearing context for future debugging

Jeremy reported "click any single per-row calendar checkbox selects
all items in the week" mid-session. His hypothesis was the Item E
publisher grouping commit. That hypothesis turned out wrong.

Root cause: piece 3's binding was `sel.has(row.cv_volume_id)`. For
Metron forward-week rows `cv_volume_id` is null, so every row's
checkbox shared the key `null`. One click → `sel = Set{null}` →
`sel.has(null) === true` on every other row.

Piece 4 had already fixed it by introducing the `cv:{id}`/`metron:{id}`
discriminator-string selection key, but Jeremy's browser was serving
cached piece-3 JS. Hard-refresh resolved.

Two methodological points worth carrying forward:
1. The vitest suite passed in jsdom because the test fixtures used
   non-null cv_volume_id values. The bug only surfaces when
   cv_volume_id is null across all rows.
2. The bug was reproduced by driving headless Chrome via the DevTools
   protocol against the live container. That technique (script at
   `/tmp/check-bug.mjs` during the session, since deleted) is the
   right move when jsdom diverges from real-browser behavior.

The two regression tests in `e2b176f` codify the invariant: clicking
one per-row checkbox in a Metron-only fixture selects exactly that
row and produces the right bulk-add payload.

## 3. System state right now

- Container `longbox` is up, healthy, on the latest build (commit
  dcd3623). Metron credentials wired, `metron_enabled = true`.
- Forward-week calendar returns 52 rows for 2026-06-03 → 2026-06-09,
  all Metron-sourced, publishers hydrated. Calendar cache populated
  earlier this session; cold-fetch wall time was ~3 min, not 60s as
  the original rate-limit model predicted. That's operational, not
  a defect — documented in the piece-3 work.
- `series.metron_id` has zero rows populated. Will fill in lazily as
  users subscribe via the Option C path; piece 3's `project_metron_items`
  has a tier-1 chain that fires off `series.metron_id` JOIN whenever
  it's populated.
- Docker Desktop VM had hit 100% disk (97.9GB used / 0 free) at the
  start of the session. Aggressive `docker image prune -a -f` freed
  27.8GB. Worth keeping an eye on across sessions.
- Metron itself had an unrelated outage during this session (TCP 443
  unreachable from the host for ~30 min). It came back, the Option C
  live probe validated against it cleanly.

## 4. What's open

**Nothing for Item A v2.** Piece 1 (longbox-metron crate), piece 2
(migration + bootstrap + kill switch), piece 3 (forward dispatch +
calendar cache + frontend), piece 4 (Option C subscription path) are
all merged with tests and regression coverage.

Adjacent open items, none urgent:
- `gcd_*` settings rows shipped as piece-2 placeholders. No GCD
  integration exists. The rows are dead-code-flagged. Remove when
  it's clear GCD won't be added.
- `metron_calendar_cache` 24h TTL is fine but the cold-fetch wall
  time of ~3 min is user-visible if the cache misses on a fresh
  range. Could be optimized by pre-warming on a schedule, or by
  shipping a "loading…" affordance in the frontend. Neither is
  worth doing speculatively.
- The pre-existing range-contains clippy warning in
  `longbox-comicvine/src/enrichment.rs:437` is the only non-clean
  clippy line in the workspace. Trivial fix when you're next in
  that file.

## 5. References

- Codebase root: `/Users/jeremy/Projects/longbox`. Multi-crate Rust
  workspace + SvelteKit frontend, hermetic Docker multi-stage build.
- Codified rule: redeploy with `docker-compose up -d --build --force-recreate`.
  New workspace crates need the Dockerfile stage-2 COPY line added in
  the same commit.
- WAL-rule sidecar pattern for live-DB mutations: stop longbox →
  `docker run --rm --volumes-from longbox alpine` with sqlite installed
  on the fly → start longbox. Used this session to clean up the
  series id=54 / 14 issues left over from the Option C probe.
- The Option C probe doc (`docs/longbox-a9-option-c-probe.md`) is the
  authoritative record of what was validated live.

## 6. Standing tone reminder

Jeremy is direct, swears freely, and is allergic to hedging. State
the answer first. Brevity beats completeness. Don't pad. Don't
narrate. When pushing back, push back hard.
