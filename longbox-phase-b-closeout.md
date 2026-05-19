# Phase B closeout: manual smoke checklist

Surfaced for closeout once the Colima mount issue from Step 5 is resolved.
The programmatic acceptance test (`cargo test -p longbox-postprocess --test
acceptance`) is the permanent regression guard; this checklist validates
everything the test can't reach — container runtime, host filesystem
semantics, real CV-tagged files, actual SAB-style drop patterns.

## Prerequisites

- Colima config exports `$HOME` mount (or whichever path holds the watch
  folder); confirm with `colima list` showing the mount and
  `docker run --rm -v $HOME:/host alpine ls /host` resolving
- Real LongBox container running against your actual library DB
  (`docker compose up` or equivalent)
- `$DOWNLOAD_WATCH_PATH` set in the container env and pointed at a
  host-mounted, writable dir
- Real library has at least 2–3 existing series in the catalog with at
  least one known issue each (pick targets you'll use for files a/b/e)

## File preparation (6 files of varying provenance)

| # | Name in watch folder | ComicInfo? | Expected outcome |
|---|---|---|---|
| a | `<known-series> 001.cbz` with real ComicInfo for that issue | ✓ | Owned import → library convention path |
| b | `<known-series> 002.cbz` (no ComicInfo) | — | Owned via filename match |
| c | `Some Unknown Series 001.cbz` with ComicInfo for unknown series | ✓ | `_unsorted/Some Unknown Series 001.cbz`, status=unmatched |
| d | `garbage_no_issue_number.cbz` (no ComicInfo) | — | `_unsorted/garbage_no_issue_number.cbz`, status=unmatched |
| e | `<series-with-existing-issue> NNN.cbz` whose target already exists | — | Conflict → cache + dashboard counter increments by 1 |
| f | `<known-other-series> 001.cbz` — drop this BEFORE container start to test initial-sweep | — | Picked up at boot, owned import |

## Drop sequence

1. **Pre-stage (f)** in the watch folder while LongBox is stopped.
2. **Start LongBox.** Wait ~5 s. Hit `/files/pending-intervention` — should
   show empty (count=0).
3. **Verify (f)** landed by hitting the dashboard — Owned counter
   increments, "Recently completed issues" activity panel shows the new
   file.
4. **Drop a, b, c, d, e** into the watch folder. Order doesn't matter;
   pipeline is serial.
5. **Wait ~10 s** (allow stability window + sequential processing).

## Verification

- **Dashboard counters:** Owned bumped by +3 (a, b, f), Unmatched bumped
  by +2 (c, d), Pending = 1 (just e).
- **Filesystem:**
  - a, b, f → moved out of watch folder into `{series} ({year})/` under
    library root with library-convention names
  - c, d → moved into `library_root/_unsorted/`
  - e → still in watch folder, untouched
  - The pre-existing target for e → bytes unchanged
- **`/files/pending-intervention`:** one row, source = e's path,
  target = library path it tried for, reason = Conflict
- **`/files?status=unmatched`:** c and d visible, plus any pre-existing
  unmatched
- **Activity feed (dashboard "Recently completed issues"):** a, b, f
  appear with timestamps

## Resolution-path check (optional, validates self-healing eviction)

1. Manually delete the conflicting source file e from the watch folder
   (or move it out).
2. Within a few seconds, the dashboard Pending counter should drop to
   0 — the notify watcher's Remove handler evicts the cache entry.

## Sign-off

If all of the above lines up, Phase B is done per the brief. Anything
that doesn't — write it up and triage as either a real bug or a Phase B+
deferral.
