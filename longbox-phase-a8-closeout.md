# Phase A.8 closeout: manual end-to-end smoke

Step 13 of the A.8 brief. Steps 1–12 are committed and the workspace
test suite is green; this run validates the full pull-to-catalog chain
against **real services** — the thing no automated test reaches.

Fill in the `Observed:` lines as you go. This is a record of what
*happened*, not a list of what *should* pass — note partial results,
surprises, and anything that needed a workaround. If something
unexpected surfaces, write it up under "Unexpected observations" at the
bottom; that section is the actual point of the doc.

- **Run date:** 2026-05-28 (resumed; original start 2026-05-26)
- **Image:** `longbox:latest` (`819ad7a0488c`) — built from commit
  `b2c8cdc` (Bug 3a Scene-format normalizer). Smoke at original
  start ran against `aacfd51e16df` / `04705ae`; the 2026-05-26
  Scenario 1 attempt surfaced three blockers (Issues A, B, C —
  see Unexpected observations) that gated resumption. Bug 3
  (`6de4d5f`, 2026-05-28) and Bug 3a (`b2c8cdc`, 2026-05-28) ship
  the Issue C fix and now sit on top of A.8 + the earlier A.9
  hot-fixes. The smoke MUST resume against this image, not the
  original — anything else would attribute results to a build
  that predates the fix. Earlier session hot-fixes: A.9 Steps 4,
  6a, 6b on top of A.8 Step 12, then `c9f0013` dedup, `00c6f9c`
  parser year-first, `ac11c3b` F6 dismiss-trap, `1db527a`
  shallow-UX, `0033af0` Bug 1a zero-attachment rollback,
  `f2ee4d5` Bug 1b three permissive parser patterns, `30d679c`
  Bug 2 phase-2 fallback, `8f84f8f` clippy hygiene. All
  reconciliation-surface; none touched the pull-to-catalog chain
  validated here.
- **Caveat:** the Bug 2 cleanup migration
  (`20260526100000_dedup_series_across_null_year.sql`) did NOT
  auto-apply on container startup; was applied via `sqlx migrate
  run` CLI + DB swap. Live DB has both migration entries and the
  cleanup result. Smoke runs against the same live DB. The
  auto-run mismatch is tracked as a deferred item.
- **Result:** ☐ pass ☐ pass-with-notes ☐ blocked

---

## Prerequisites

- [x] Container deployed from the current image — `docker-compose up -d
      --force-recreate` (`longbox` on `d69d5c13d809`, `/api/health` 200).
- [x] `COMICVINE_API_KEY` set; `LIBRARY_ROOT_PATH=/library` and
      `DOWNLOAD_WATCH_PATH=/watch` point at host-mounted, writable dirs.
      Colima mount confirmed (614 entries visible host-side); the VM
      view was re-synced via `colima restart` at 13:14 PDT 2026-05-22
      after a stale-mount blip on the 6b deploy.
- [x] A real Newznab indexer configured + enabled in Settings.
      (Prowlarr, priority 0, maxage 1500 d.)
- [x] A real downloader (SABnzbd or NZBGet) configured + enabled, its
      category pointing at the watch folder.
      (SAB @ `192.168.1.163:8081`, category `7030`.)
- [x] At least one webhook configured (Slack or other) subscribed to
      `pull_failed` + `pull_engine_error`.
      (Slack webhook "longbox", `event_mask=15` = `EVENT_MASK_ALL`.)
- [ ] Catalog has a series with a genuinely-available solicited issue
      (something the indexer can actually find an NZB for).
      19 pull-list entries already seeded; pick one whose next solicited
      issue Prowlarr can actually resolve.

> **Mount fragility — permanent note.** The container's `/library` is a Colima
> virtiofs bind mount of the host `/Volumes/Comics`. Any host-side unmount or
> remount of that volume strands the running Colima VM on a stale view: the
> container keeps a broken/empty `/library` until the VM re-syncs. After **any**
> unmount/remount of `/Volumes/Comics`, run `colima restart` before trusting a
> scan, reconcile, or smoke run. (Surfaced 2026-05-21 during the A.8 deploy
> recovery — `docker-compose up` failed with `mkdir /Volumes/Comics: file
> exists` until the VM was restarted.)

Environment notes (versions, hosts, anything non-obvious):

> - Host: 16" MacBook Pro M5 Max / macOS Tahoe 26.4.1 / Colima virtiofs
>   (aarch64). Container `longbox:latest` = `cc251e57578f`, commit
>   `c9f0013`. Version string `0.0.1`.
> - Catalog at smoke start: 377 series, 0 phantom-transition rows, 0
>   untracked folders, 19 pull-list entries. (Down from 640 series /
>   563 untracked folders pre-hot-fix — combination of the dedup
>   cleanup migration deleting 8 dupes, the user's manual link-mode
>   re-convert pass on Pattern A folders, broader manual tidy on the
>   "Empty series" backlog, and F6 auto-dismissing stale
>   `discovered_folders` rows on a post-deploy scan.)
> - Pattern A re-convert outcome NOT fully verifiable from data
>   alone: Batwoman (56) and Narco (60) still show `owned=0, total=3`
>   (CV issues present, no files attached); Adventureman (76) and
>   A Haunted Girl (70) absent from the catalog entirely. Smoke
>   proceeds because the pull-to-catalog chain is independent of
>   those four folders' state.
> - A.9 6b auto-tidy is enabled and ticking, but with N=3 + 14-day
>   window nothing can be marked or purged inside this smoke run.

---

## Scenario 1 — pull-to-catalog happy path

Add a series to the pull list, let a sweep run, and confirm the issue
flows all the way into the catalog attributed to the pull engine.

1. Open a series detail page, subscribe it to the pull list.
2. Trigger a sweep — `/releases/pull-list` → "Check now" — or wait for
   the scheduled `PULL_SCHEDULE_TIME` slot.
3. Watch the downloader queue, the watch folder, then the catalog.

- [ ] Sweep ran (check logs / the pull-list "last pull" column).
- [ ] NZB submitted to the downloader — appears in its queue.
- [ ] Download completed; file landed in the watch folder.
- [ ] Phase B caught the file and moved it into the library.
- [ ] The issue is `owned` in the catalog.
- [ ] **`match_method = 'pull_list'`** on the resulting file row.

Observed:

> ____

---

## Scenario 2 — failure paths

Each failure should surface on `/needs-attention` and (where wired) fire
a webhook.

### 2a — grab failure (kill the download mid-flight)

1. Start a pull as in Scenario 1; while it is downloading, stop SAB /
   NZBGet (or delete the job) so the grab cannot complete.
2. Let the engine poll it across sweeps to the retry cap.

- [ ] The attempt transitions to `failed` after retry exhaustion.
- [ ] It appears on `/needs-attention` under "Pull failures",
      categorised **Grab failed**.
- [ ] A `pull_failed` webhook was delivered.
- [ ] "Retry" on that row un-parks the issue; the next sweep re-attempts it.

Observed:

> ____

### 2b — bad indexer API key

1. Edit the indexer to use an invalid API key.
2. Trigger a sweep.

- [ ] The sweep handles the indexer error without crashing.
- [ ] A `pull_engine_error` webhook was delivered (if the failure is
      engine-wide) — or note that it was a per-issue error instead.
- [ ] Restoring the key + re-sweeping recovers cleanly.

Observed:

> ____

### 2c — submission failure (downloader unreachable)

1. With a found release, make the downloader unreachable (stop it, or
   point it at a dead host).
2. Trigger a sweep.

- [ ] The attempt records as `failed` with no `release_id`.
- [ ] It appears on `/needs-attention` categorised **Submission failed**.

Observed:

> ____

---

## Scenario 3 — webhook delivery

- [ ] Settings → a webhook's "Test" button delivers a test notification.
- [ ] A Slack-host webhook renders as a formatted block-kit message.
- [ ] A non-Slack webhook receives the plain `{ event, message }` JSON.
- [ ] A real `pull_failed` / `pull_engine_error` from Scenario 2 arrived
      at the configured target.

Observed:

> ____

---

## Catalog-math invariant (post-smoke)

After the smoke run, with the catalog at rest:

- [ ] Auto-pulled files all carry `match_method = 'pull_list'`.
- [ ] Dashboard counters reconcile — owned / needs-review / unmatched /
      missing still sum consistently against the catalog.
- [ ] No phantom or duplicate rows introduced by the pull → Phase B path.

Observed:

> ____

---

## Done-definition gate summary

- [ ] 1. All 13 steps committed, locked decisions honored.
- [ ] 2. Workspace tests green (already true at Step 12).
- [ ] 3. End-to-end smoke validates the full pull-to-catalog chain.
- [ ] 4. Catalog-math invariant verified post-smoke.
- [ ] 5. Webhook delivery confirmed against a real target.
- [ ] 6. Brief + this closeout doc tracked in the repo.

---

## Unexpected observations

Anything that didn't match expectations — surprises, workarounds,
follow-up bugs, deferred items that turned out to matter. If this
section stays empty, the run was clean.

### Smoke-run blockers (Scenario 1, 2026-05-26)

Scenario 1's pull-to-catalog chain surfaced three blockers before
the chain could complete. Tracked here so the closeout has a
canonical home for them — A and B are config-side and need a
deployment decision; C is a code bug being fixed in Bug 3.

- **Issue A — no filesystem bridge between SAB output and the
  LongBox watch folder.** SAB runs on a remote Windows machine
  (`192.168.1.163`) and writes completed downloads to
  `C:\Users\jerem\Downloads\complete\`. The LongBox watch folder
  is on the Mac at `/Users/jeremy/longbox-phase-b-watch/`. No
  filesystem bridge exists. Resolution paths (Jeremy to pick):
  (1) install SAB locally on the Mac and point `comics` category
  complete_dir at the watch folder; (2) set up SMB/NFS between
  Windows SAB output and Mac watch folder; (3) pause Scenario 1
  and run Scenarios 2 + 3 first, return to Scenario 1 once a
  bridge is in place. **Status:** open, awaiting decision before
  smoke resumes.

- **Issue B — SAB ignores LongBox's `category=7030`.**
  `downloader_config.category` is set to the newznab cat code
  (`7030`), but SAB's valid category names are
  `*, movies, comics, books, tv, prowlarr, music`. SAB falls back
  to the default `*` category silently. **Resolution:** edit the
  downloader config in /downloader UI, set `category = "comics"`.
  One-click config change, no code fix needed. **Status:** open,
  trivial fix pending deploy of Bug 3.

- **Issue C — newznab `select_best` grabs wrong-series releases.**
  Pull for "Odin 1" grabbed "Beware the Eye of Odin 001"; pull
  for "The Darkness 1" grabbed "Justice League - Road To Dark
  Crisis 001". Indexer's full-text returns partial-match results
  and `select_best` ranked them only by format + grabs + recency
  — no series-title similarity check. Compounded by the pull
  engine not passing `year` to `find_release_excluding`, so
  volume disambiguation was off too. **Status: RESOLVED** by
  Bug 3 (`6de4d5f`, 2026-05-28) + Bug 3a (`b2c8cdc`, 2026-05-28).
  Bug 3 added a series-title similarity post-filter at threshold
  0.75 (catalog-matcher primitive), passed year from
  `series.start_year` to narrow at the indexer server-side, and
  wired the `'mismatched'` `pull_attempts` status (the previously-
  reserved-but-unused enum value) surfaced on `/needs-attention`
  under a new `series_mismatch` category. Bug 3 verify (live,
  same day) confirmed against the exact 5-26 guid that the
  wrong-grab is now rejected — and immediately surfaced an
  over-rejection problem: real Prowlarr responses use Scene
  naming (`Beware.the.Eye.of.Odin.001.2022.Digital.Mephisto-Empire`)
  that fails `parse_filename`'s extension-bearing canonical
  patterns, so every legit grab would also reject as unparseable.
  Bug 3a added a Scene-format normalizer (dots→spaces, wrap
  rightmost in-range bare year, append `.cbz`) as a fall-back
  after raw parse failure. 11/13 archaeology cases parse
  correctly under the normalizer; the two graceful failures
  (special-edition annotation between number and year; no-year
  titles) fall to the existing unparseable→mismatch path. Bug 3a
  verify confirmed the diagnostic transition on the same Odin
  fixture: error_message went from `"indexer returned 1 results,
  none parseable as a comic release"` (Bug 3) to `"indexer
  returned 1 results, 1 parseable, best similarity 0.20 vs
  requested \"Odin\" below threshold 0.75"` (Bug 3a) — the wrong
  grab is now rejected by similarity, not by the parser bouncing
  the format. See `longbox-phase-a9-prompt.md` Deferred items
  for derivatives: park lifecycle, no-year wrong-volume
  residual, Scene editorial-annotation tokens.

**Smoke resumes when:** A is resolved (Path 2 SMB share
reconnect on the host), B is fixed (one click in /downloader to
set `category = "comics"`), C is shipped (Bug 3 + 3a, done). C is
green. A and B are user-driven operational steps.

