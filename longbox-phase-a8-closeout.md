# Phase A.8 closeout: manual end-to-end smoke

Step 13 of the A.8 brief. Steps 1–12 are committed and the workspace
test suite is green; this run validates the full pull-to-catalog chain
against **real services** — the thing no automated test reaches.

Fill in the `Observed:` lines as you go. This is a record of what
*happened*, not a list of what *should* pass — note partial results,
surprises, and anything that needed a workaround. If something
unexpected surfaces, write it up under "Unexpected observations" at the
bottom; that section is the actual point of the doc.

- **Run date:** 2026-05-26
- **Image:** `longbox:latest` (`aacfd51e16df`) — built from commit
  `04705ae` (A.9 deferred-items doc, on top of Bug 2 cleanup
  `30d679c`). A.9 Steps 4, 6a, 6b shipped on top of A.8 Step 12,
  followed by five hot-fixes (`c9f0013` dedup, `00c6f9c` parser,
  `ac11c3b` F6 dismiss-trap, `1db527a` shallow-UX, then in this
  session: `0033af0` Bug 1a zero-attachment rollback, `f2ee4d5`
  Bug 1b three permissive parser patterns, `30d679c` Bug 2 phase-2
  fallback) plus a clippy hygiene commit (`8f84f8f`). All
  reconciliation-surface changes, none of which touch the
  pull-to-catalog chain this doc validates.
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

> ____
