# LongBox state-of-system snapshot

Date: 2026-06-02, late.
Audience: future-Claude / Jude picking up the LongBox thread.
Supersedes: `longbox-state-of-system-2026-06-01-late.md` (in tree;
relevant for context older than today). Earlier snapshots are in
history.

## 1. What closed this session

Fifteen commits forming three coherent arcs — **pull-engine
correctness archaeology**, **digital-edition exclusion**, and
**Library Tidy enrichment review**. Plus drive-by hardening and
operational fixes.

```
122cdcd pull engine: series Search button works without a pull-list subscription
661e4c9 Library Tidy: enrichment-review queue end-to-end
9bbe0db series detail: per-series "Search missing" button in the header
306a3ca cv-enrichment: widen year gate from exact to ±1 year
609e2e2 pull engine: extend exclusion keywords with collected-edition formats
5f4d8bc cv-enrichment: penalize collected-edition candidates (TPB / HC / Omnibus)
cd805fc pull engine: title exclusion keywords for digital-only formats
405455c pull engine: drop year_hint from indexer searches entirely
36575c8 a9 pull engine: drop year_hint — CV dates mismatch NZB scan years
2061edd pull engine: derive year_hint from issue.cover_date, not series.start_year
ee111fa newznab: append .cbz before scene-normalizing space-separated titles
585e10b newznab: stop embedding the series start_year in the q-term
c19ee80 a9 newznab: parallel fan-out across all indexers, best-result-wins
16a6f86 cv-enrichment: download cover_url as folder.jpg into series library dir
c31728c Phase B: remove empty watch-folder subdirs after successful import/unsort
```

### Arc 1: pull-engine correctness archaeology

The session opened on a single user-reported symptom — NZBGeek
returned 85 results for "Beneath the Trees Where Nobody Sees" via
Prowlarr but LongBox got zero. The chase took five separate
commits as each layer of misdiagnosis peeled back to reveal the
next.

**ea49725 (prior session) — Mylar3 User-Agent**. The first guess:
NZBGeek allowlists by UA, generic `reqwest` UA gets blocked.
Real, but partial — once the UA fix landed, the indexer DID
return results (proven by `mismatched` pull_attempts), they just
all failed the downstream pre-grab filter for different reasons.

**585e10b — strip year from the q-term**. Newznab `t=search`
does a literal substring match on `q`. The previous shape was
`{series} {issue} (YYYY)` using `series.start_year`. NZBs are
tagged with their RELEASE year, not the series start_year, so a
literal `(2023)` from the catalog never appeared in NZBs tagged
`(2024)` for any ongoing series. Prowlarr's 85-vs-0 result delta
proved year-embedding was the killer. The year stays on the
similarity-filter side (it ranks results); just not in the
indexer text query.

**ee111fa — `.cbz`-append before scene-normalizing**.
`parse_release_title` ran `raw → normalize_scene_title` as its
fall-back. NZBGeek's space-separated titles like
`Beneath the Trees Where Nobody Sees 001 (2023) (digital) (Son of Ultron-Empire)`
failed the raw parse on the missing extension, then hit the
normalizer. The normalizer's rightmost-year wrap regex
`\b(19[5-9]\d|20[0-3]\d)\b` matches inside an existing `(2023)`
because `\b` fires at the paren↔digit edge, so `(2023)` became
`((2023))` and pattern 10's year capture rejected the input.
Fix: a middle pass that tries `format!("{title}.cbz")` before
the normalizer. Just satisfies the extension anchor — no token
transforms. Scene-format dotted titles still hit the full
normalizer on the final fall-through. Order is correctness-
critical and documented as load-bearing.

**2061edd → 405455c → 36575c8 — the year_hint death march**.
First fix: derive year_hint from `issue.cover_date` instead of
`series.start_year`, since NZBs carry per-issue years, not
volume start years. Test data updated accordingly. **Deployed.
Still no grabs.** Second pass: drop year_hint to None
entirely. The misdiagnosis chain was: CV records `cover_date`
as the printed cover-month of the physical issue, but NZB
titles carry the SCAN year — late physical scans of older
digital releases, re-rips of back issues, and off-by-one
solicitation windows all routinely disagree by a year. Real
example from this catalog: CV records 2024-09-01 for an issue
NZBs tag as (2023). With the pull engine already gated on
specific `(series_id, issue_id)`, the series-title similarity
filter carries the entire wrong-volume disambiguation load.
Year was at best redundant and at worst correctness-destroying.

The `36575c8` commit was Jeremy's own pass at this — the
sequence shows the iterative archaeology pattern that
characterized the day.

### Arc 2: digital-edition exclusion (two-layer defense)

After Arc 1 landed, NZBGeek hits were reaching the similarity
filter. The next failure mode: a subscription to "Amazing
Spider-Man" was happily grabbing Marvel Infinity Comics (the
digital-vertical-scroll exclusives, not the print issues).
Same shape for DC Infinite Comics. Then a second failure:
the enrichment worker mis-linked "Beneath the Trees Where
Nobody Sees" to CV volume 160379 — a TPB with
`count_of_issues=1` and `description="Trade paperback collecting
issues 1-6 of..."` — instead of the 6-issue original (154239).

Two-layer fix:

**5f4d8bc — cv-enrichment collection penalty**. New
`is_collection_volume(r)` detects TPB/HC/Omnibus candidates via
name terms (whole-word check), description boilerplate
("trade paperback", "hardcover", "omnibus", "collected
edition"), and the `issue_count==1 + "collecting"` boilerplate
signal. Detected candidates get a `COLLECTION_PENALTY = 0.5`
multiplier applied at the scoring stage, BEFORE the title-
threshold and dominant-gap gates. At the 0.85 year-known
threshold, a raw-1.0 collection lands at 0.5 and is title-
rejected outright — the original always wins. Collection-only
pools become LowConfidence and surface for manual pick.

**cd805fc + 609e2e2 — pull-side exclusion keywords**. New
`pull_exclusion_keywords` setting (CSV, default
`Infinity Comic,Infinite Comic,Trade Paperback,Hardcover,
Omnibus,Compendium`). `filter_by_series_title` pre-filters the
release pool before counting `total_results` — load-bearing
so an all-excluded indexer pool returns silent no-match
rather than a spurious series-mismatch diagnostic. Match is
normalized (lowercase + dots → spaces) so a single
human-readable keyword covers NZBGeek-style space-separated
AND Scene-format dotted shapes.

Result: enrichment can't auto-link a subscription to a TPB,
AND even if a hand-pulled subscription points at a TPB-flavored
release, the indexer pre-grab filter drops it before submission.

**306a3ca — Jeremy's drive-by year-gate widening**. Independent
of mine — widened the enrichment year gate from exact match to
±1 year, since CV's solicitation-month cover_date frequently
lands the wrong side of a year boundary from publication.

### Arc 3: Library Tidy enrichment review

By session end, the catalog had ~209 shallow series the
enrichment worker had refused to auto-link (multi_match /
low_confidence / year_mismatch / collision_disabled / error)
— more after the Arc 2 collection penalty started rejecting
TPB auto-matches. The user needed a way to resolve these
without per-series manual SQL.

**9bbe0db — header "Search missing" button on series detail**.
Bulk equivalent to the existing per-issue Search button on each
Missing row. Mirrors IssueRow's `missing` derivation exactly
(`!file && !isSolicited(cover_date)`) so the buttons surface
and disappear in lockstep.

**661e4c9 — Library Tidy enrichment-review queue
(end-to-end, 4 parts)**. Backend:
`GET /api/library/tidy/enrichment-queue` returns the
review-actionable shallow set with title + start_year +
outcome + diagnostic + owned_count, sorted by impact
(`owned_count DESC, title ASC`). `PATCH /api/series/:id/cv-id`
takes a user-picked cv_id, pre-checks UNIQUE collision (clean
409), fetches the CV volume + issues BEFORE any DB write,
then in a transaction: overwrites cv_id via new
`force_set_cv_id` (the existing `set_cv_id` is CAS-style on
`cv_id IS NULL`, wrong semantics for an explicit user pick),
overwrites descriptive fields, wipes issues, bulk-inserts
new issues. Triggers auto-rematch so files attached to the
deleted issues get re-matched. Frontend: typed
`$lib/api/enrichment.ts` wrapper module + parallel-loaded
queue in `+page.ts`. New "Enrichment needs review" section
between "Untracked folders" and the bottom, gated on
`enrichmentQueue.length > 0`. Per-row: title (year) +
outcome badge (amber/slate/blue/red/red per outcome) +
owned-file count + inline `CvSearchInput`. Pick → PATCH →
optimistic local removal + success toast. Per-row in-flight
Set so one pick doesn't lock out the rest.

**Crucial UX detail**: `CvSearchInput` is rendered WITHOUT
`initialQuery`. At ~200+ queue rows, passing the series
title would fire ~200 concurrent CV searches at page mount
and blow through CV's 180/hour rate limit. User types
per-row instead.

**122cdcd — series Search button works for unsubscribed
series**. Bug fix: `POST /api/pull/search/:series_id` was
gating on `pull_list_repo::get` and 404'ing every
unsubscribed series, which made the header button (commit
`9bbe0db`) functionally broken on the common case. Rewired
to: 404 only if series doesn't exist, query missing issues
via new `issue_repo::list_missing_for_series`, fan out
`fire_issue_search` per missing issue. Returns
`202 {queued: N}` or `200 {queued: 0, note: "..."}`.

### Ancillary work

**c31728c — empty watch-folder subdir cleanup**. SABnzbd
deposits as `{watch_root}/{job_name}/{file.cbr}`. Phase B was
moving the file but leaving the empty job dir behind forever.
Now: after a successful Imported/Unsorted outcome, the
consumer task calls `cleanup_empty_parent` — canonicalized
equality check against watch_root protects the root itself,
`read_dir` count protects non-empty parents. Best-effort
throughout.

**16a6f86 — folder.jpg from CV cover_url**. After successful
enrichment, download the series's CV cover image and write
it as `folder.jpg` in the series's library folder — same
file Plex/Komga/Finder pick up for offline cover art, same
file Mylar3 has always written. Four refusal guards: NULL
cover_url, folder doesn't exist on disk yet, `folder.jpg`
already exists (don't overwrite Mylar-generated covers),
HTTP failure. All silent or warn-log. Called post-tx so the
filesystem side-effect can't poison a rolled-back enrichment.

**c19ee80 — parallel indexer fan-out** (Jeremy's). Reshaped
`find_release_excluding_filtered` to fan out across all
configured indexers concurrently via `futures::join_all`
instead of sequential priority order. Best release from the
combined pool wins. Priority becomes the tiebreaker for
equal-grabs/format candidates. A single slow or empty
indexer no longer blocks the others — Prowlarr-style.

## 2. System state

- Container `longbox` is up, healthy in 1s, on commit
  `122cdcd`. Boot tail clean: pool open, Phase B watcher
  started polling `/watch` every 30s, CV enrichment started,
  listening on `:3000`. Zero warn/error lines on boot.
- Pull engine: daily scheduler running, manual `/pull/check`
  available, per-series `/pull/search/:id` works for ANY
  series (subscribed or not), per-issue
  `/pull/search/:series_id/issue/:issue_id` available.
  Year-gate disabled in `q`-term AND in the year-hint;
  similarity filter alone carries wrong-volume rejection.
  Parallel indexer fan-out across all enabled indexers.
- CV enrichment worker: running, with collection-edition
  penalty and ±1-year gate active. Cover image downloads
  trigger post-merge for successful Matched/PartialMerge
  outcomes.
- Phase B: PollWatcher on virtiofs (30s interval); writes
  ComicInfo.xml + MetronInfo.xml on import; empty
  watch-folder subdirs cleaned up; mount-health preflight
  guards the scanner.
- Library Tidy page now has three sections (phantom series,
  untracked folders, enrichment needs review) plus the
  bulk-convert path. Empty state requires all three surfaces
  empty.
- Forward-week calendar still serving Metron-sourced rows
  from the morning's Item A v2 cache (24h TTL).

## 3. What's open

**Pull engine on a fresh-start catalog**. The morning's
container-and-volume loss (see Standing concerns below) wiped
the catalog. Jeremy rebuilt by re-subscribing the priority
list. Real-world validation of the Arc 1 + Arc 2 fixes is
still mid-flight — the engine is fan-outing across indexers
and the filter is doing the right shape of work, but
end-to-end "issue lands in `/library/`" against the rebuilt
catalog hasn't been observed at scale yet. Next sweep is the
real test.

**Enrichment queue UX at 200+ rows**. The Library Tidy
enrichment-review section renders inline CvSearchInputs per
row with empty `initialQuery` (no auto-fire — would blow
through CV's 180/hour limit). The user types per row. At
high queue counts this is tedious; a future iteration could
add a "Run auto-pick again" button that re-attempts the
worker pass after the user has resolved a chunk, since
resolving high-owned-count rows might tip the dominant-gap
guard on related ones.

**Collection-penalty observation period**. The
`COLLECTION_PENALTY = 0.5` multiplier means any TPB at raw
score 1.0 lands at 0.5, below the 0.85 year-known threshold.
Aggressive by design — wrong auto-link to a TPB is
asymmetrically worse than refusal. Watch for legitimate
single-volume graphic novels (e.g., a real one-shot OGN with
`count_of_issues=1`) landing as LowConfidence when they
should match. The description-based detector should avoid
this (real OGNs don't say "trade paperback collecting" in
their CV description) but reality often disagrees.

**Cargo.lock futures-dep cleanup** is now closed (was open
in the prior snapshot; lock got regenerated this session as
a side effect of cv-enrichment's reqwest add).

**Pre-existing clippy warning in
`longbox-comicvine/src/enrichment.rs:437`** is now closed
(drive-by fix in `5f4d8bc`).

## 4. Codified rules carried forward from this session

- **`cargo sqlx prepare` is gated.** Auto-mode classifier
  blocks it unless authorized. To use safely: spin a fresh
  temp DB, apply all migrations, point `DATABASE_URL` at it,
  then run prepare with `-- --all-targets` (the bare command
  skips test code and leaves a partial cache):

  ```
  rm -f /tmp/longbox-prepare.db
  DATABASE_URL="sqlite:/tmp/longbox-prepare.db?mode=rwc" \
    sqlx migrate run --source longbox-db/migrations
  DATABASE_URL="sqlite:/tmp/longbox-prepare.db" \
    cargo sqlx prepare --workspace -- --all-targets
  SQLX_OFFLINE=true cargo build --workspace --tests
  ```

  Verify the cache delta is additive only (new entries; zero
  deletions of macros still in source) before committing.

- **Misdiagnosis is the failure mode, not bugs.** Arc 1's
  five commits each fixed something real, but only the last
  one actually solved the user's problem. Pattern to watch:
  a fix lands, deploy succeeds, next sweep still fails, but
  the failure surface has SHIFTED — that's the previous fix
  WORKING, exposing the next layer. The instinct to "double-
  check the fix landed correctly" misreads the situation;
  the right move is "what failure mode is exposed now?"

- **Year is a hostile signal in pull search.** Across three
  layers (q-term, year-hint year-gate, cv-enrichment year-
  gate) the data sources disagree by a year often enough
  that strict-equality filters silently kill correct
  results. The pattern is: catalog cover_date is the
  printed cover-month of the physical issue, NZB titles
  carry the SCAN year, CV's `start_year` is the volume's
  launch year. Three different definitions of "year". Any
  strict-equality gate between them is wrong; widen to ±1
  (cv-enrichment did this) or remove (pull engine did this).

- **The collection-volume detection vocabulary.** When CV
  records a TPB / HC / Omnibus / Compendium, three
  consistent shapes appear: name contains the format word
  ("Omnibus", "TPB", etc.) as a whole word; description
  contains the format word in any context; or
  `issue_count==1 + description contains "collecting"`.
  Codified in `is_collection_volume` in
  `longbox-comicvine/src/enrichment.rs`.

- **CV rate limit is 180 calls/hour.** A 200+ row queue
  cannot afford to auto-fire one CV search per row at page
  mount. `CvSearchInput` defaults `initialQuery=''` which
  prevents auto-fire; pass it deliberately only when there's
  exactly one input on the page (modal flow).

- **`pull_attempts` has no UNIQUE on (series_id, issue_id).**
  Retry history requires multiple rows per pair. Any code
  path that submits a new attempt MUST guard against
  in-flight duplicates explicitly — the schema won't absorb
  collisions. Carried forward from prior session.

- **Redeploy is still `docker-compose up -d --build
  --force-recreate`.** Every time. New workspace crates
  still need the Dockerfile stage-2 COPY line added in the
  same commit.

## 5. Standing concerns

**The morning's Docker Desktop volume loss**. Between two
sqlite probe calls during a pull-sweep attempt, the
`longbox` container and the `longbox_longbox-data` volume
both disappeared. No `docker rm`, `docker-compose down`,
`docker system prune`, or `docker volume rm` was issued —
likely Docker Desktop's reaper or an OOM event in the
underlying VM. The volume came back as `Created` rather than
`Reusing` on the next compose up; the DB inside was
freshly-migrated and empty. 61 subscribed series + all
issues + pull history + match-method history + metron
calendar cache: gone. No backup; SQLite WAL files live in
the volume that disappeared. Jeremy moved forward by
re-subscribing the priority list rather than attempting
recovery — accepting the loss of file-match history and
needs-review judgments. The new catalog has been building
up across the rest of the day's work.

Risk: this can happen again. SQLite-in-a-Docker-volume on
Docker Desktop for macOS is not a robust persistence
substrate. A bind mount to a host path would survive Docker
Desktop VM resets. Worth raising next session if Jeremy
hasn't already moved on it.

## 6. Standing tone reminder

Jeremy is direct, swears freely, allergic to hedging. State
the answer first. Brevity beats completeness. Don't pad.
Don't narrate. When pushing back, push back hard.

Memory rules: refers to chat-Claude as "Jude"; CFO is
Melissa Pihl (full name in public copy, "Mel" private-only).
Public-facing copy uses real names and roles.
