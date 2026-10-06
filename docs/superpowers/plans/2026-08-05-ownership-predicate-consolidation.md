# Ownership Predicate Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every hand-written "is this issue owned?" SQL predicate with a single `issue_ownership` view, so PR 2 can add a `covered` state by editing one definition instead of a dozen call sites.

**Architecture:** One migration adds a SQLite view exposing `issue_id`, `series_id`, `is_owned`. Call sites that ask the boolean ownership question join it. Sites that ask a *different* question (per-status breakdowns, Library Tidy's emptiness check, the orphan-row detector) are left alone with a comment saying why. Behaviour must not change; the proof is that the missing count, dashboard payload and pull-candidate counts are identical before and after.

**Tech Stack:** Rust, sqlx 0.7 with compile-time checked queries and a committed `.sqlx/` offline cache, SQLite, `scripts/prepare-sqlx.sh`.

---

## Critical context for the implementer

**The predicate is not uniform.** Grepping `status = 'owned'` returns 38 hits. Only some are the ownership question. Classified 2026-08-05:

### CONVERT — the boolean ownership question (12 sites)

| File | Lines | Form |
|---|---|---|
| `longbox-db/src/issue_repo.rs` | 147, 186 | `NOT EXISTS` |
| `longbox-db/src/creator_repo.rs` | 51 | `EXISTS` (positive) |
| `longbox-db/src/pull_attempt_repo.rs` | 297, 324 | `NOT EXISTS` |
| `longbox-web/src/routes/pull.rs` | 211 | `NOT EXISTS` |
| `longbox-web/src/routes/missing.rs` | 93, 121, 149, 175 | `NOT EXISTS` |
| `longbox-web/src/routes/stats.rs` | 78, 85 | `NOT EXISTS` |
| `longbox-db/src/series_repo.rs` | 467, 475, 554, 562 | `NOT EXISTS` inside `COUNT(DISTINCT CASE …)` |

### DO NOT CONVERT — different questions

| File | Lines | Why |
|---|---|---|
| `longbox-db/src/series_repo.rs` | 452, 539 | `CASE WHEN f.status = 'owned'` is a **per-status breakdown** (`owned_count` sits beside `needs_review_count`, `ignored_count`, `unmatched_count`). Not a boolean ownership test. Converting it would break the breakdown. |
| `longbox-db/src/series_repo.rs` | 732, 853 | Library Tidy: `status IN ('owned','needs_review','unmatched')`, answering "does this series have *any* file on disk" for `consecutive_empty_scans` / `auto_tidy_due_at`. |
| `longbox-db/src/series_repo.rs` | 820, 826 | `UPDATE series SET last_matched_count` — counts owned FILES, not owned ISSUES. Different cardinality. |
| `longbox-db/src/series_repo.rs` | 958 | `LEFT JOIN (… GROUP BY i.series_id)` producing `owned_count` per series for enrichment. Aggregate, not a per-issue boolean. |
| `longbox-web/src/integrity_scan.rs` | 317, 548 | Orphan-row detector: `issue_id IS NULL AND status = 'owned'`. Detects breakage. |
| `longbox-db/src/opds_repo.rs` | 191, 228 | `ORDER BY (f.status = 'owned') DESC` — a sort key. |
| `longbox-db/src/file_repo.rs` | 353, 568 | A doc comment and an `UPDATE … SET status = 'owned'` write. |
| `longbox-db/migrations/*.sql` | — | Historical. Never rewrite a migration. |

`longbox-postprocess/src/processor.rs` contains no `status = 'owned'` occurrence; earlier notes listing it were wrong.

**MIGRATION STALENESS — read before running any test after a migration change.**
`sqlx::migrate!` embeds migrations into the binary at COMPILE time, and
`longbox-db` has no `build.rs`. Adding, removing or editing a migration file
does **not** reliably trigger a rebuild, so `cargo test` can silently run a
stale binary and give a confident wrong answer. Observed during Task 1:
restoring a deleted migration left the test failing until the crate was forced
to recompile.

After ANY migration change, force the rebuild before testing:

```bash
touch longbox-db/src/lib.rs
```

This matters most in Task 8, where the whole point is reading a mutation
result. A stale binary there would report "the mutation changed nothing" and
look exactly like a passing verification.

**Standing rules for every task:** run `cargo fmt --all`, `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings`, and `SQLX_OFFLINE=true cargo test --workspace` before each commit. Any task touching SQL must re-run `./scripts/prepare-sqlx.sh` and commit `.sqlx/`.

---

### Task 1: Add the view and prove it agrees with the hand-written form

**Files:**
- Create: `longbox-db/migrations/20260805000000_add_issue_ownership_view.sql`
- Create: `longbox-db/tests/issue_ownership_view.rs`

- [ ] **Step 1: Write the migration**

```sql
-- One definition of "does the catalog own this issue?".
--
-- Before this view the predicate was hand-written in a dozen queries
-- across four crates. Nothing was broken -- the copies agreed -- but
-- adding a third ownership state means editing every copy correctly,
-- and "edit all N copies correctly" is what failed twice in one week
-- here: three separator normalisers where only one knew about `_`, and
-- two digest-freshness rules where only one validated against disk.
-- Both shipped as silent failures.
--
-- `is_owned` is SQLite integer 0/1 and sqlx types it i64; call sites
-- compare `= 1` explicitly so the storage type stays visible.
--
-- Verified 2026-08-05 against a copy of the live catalog: SQLite
-- inlines this view completely, so the query plan is unchanged and the
-- covering index from 20260608000000_add_dashboard_stats_indexes.sql
-- is still used.
CREATE VIEW issue_ownership AS
SELECT i.id        AS issue_id,
       i.series_id AS series_id,
       EXISTS (SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1) AS is_owned
FROM issues i;
```

- [ ] **Step 2: Write the failing equivalence test**

```rust
//! The view must answer exactly what the hand-written predicate
//! answered. This is the whole risk of the consolidation: not "does the
//! view work" but "did meaning change".

use longbox_db::Pool;

async fn seed(db: &Pool) {
    sqlx::query("INSERT INTO series (id, title, sort_title) VALUES (1, 'S', 's')")
        .execute(db).await.unwrap();
    for (id, num) in [(1, "1"), (2, "2"), (3, "3"), (4, "4")] {
        sqlx::query("INSERT INTO issues (id, series_id, number) VALUES (?, 1, ?)")
            .bind(id).bind(num).execute(db).await.unwrap();
    }
    sqlx::query("INSERT INTO library_roots (id, path) VALUES (1, '/x')")
        .execute(db).await.unwrap();
    // issue 1: owned+present -> owned
    // issue 2: owned but absent -> NOT owned
    // issue 3: present but needs_review -> NOT owned
    // issue 4: no file at all -> NOT owned
    for (fid, iid, status, present) in [
        (1, Some(1), "owned", 1),
        (2, Some(2), "owned", 0),
        (3, Some(3), "needs_review", 1),
    ] {
        sqlx::query(
            "INSERT INTO files (id, issue_id, library_root_id, path_relative, size_bytes,
                                mtime, last_scanned_at, match_method, match_confidence,
                                status, is_present, last_seen_at)
             VALUES (?, ?, 1, 'p' || ?, 1, datetime('now'), datetime('now'), 'test', 1.0, ?, ?, datetime('now'))")
            .bind(fid).bind(iid).bind(fid).bind(status).bind(present)
            .execute(db).await.unwrap();
    }
}

#[tokio::test]
async fn the_view_agrees_with_the_hand_written_predicate() {
    let db = longbox_db::open(":memory:").await.unwrap();
    seed(&db).await;

    let hand: Vec<i64> = sqlx::query_scalar(
        "SELECT i.id FROM issues i
         WHERE NOT EXISTS (SELECT 1 FROM files f
                           WHERE f.issue_id = i.id
                             AND f.status = 'owned' AND f.is_present = 1)
         ORDER BY i.id")
        .fetch_all(&db).await.unwrap();

    let view: Vec<i64> = sqlx::query_scalar(
        "SELECT issue_id FROM issue_ownership WHERE is_owned = 0 ORDER BY issue_id")
        .fetch_all(&db).await.unwrap();

    assert_eq!(hand, vec![2, 3, 4], "fixture sanity: 2,3,4 are missing");
    assert_eq!(view, hand, "the view must not change meaning");
}
```

- [ ] **Step 3: Run it and verify it FAILS**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db --test issue_ownership_view`
Expected: FAIL — `no such table: issue_ownership`. If it passes, the migration already ran; stop and investigate.

- [ ] **Step 4: Regenerate the offline cache so the migration is applied**

Run: `./scripts/prepare-sqlx.sh`
Expected: `SQLx offline cache regenerated under .sqlx/`

- [ ] **Step 5: Run the test again and verify it PASSES**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db --test issue_ownership_view`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
SQLX_OFFLINE=true cargo test --workspace
git add longbox-db/migrations/20260805000000_add_issue_ownership_view.sql \
        longbox-db/tests/issue_ownership_view.rs .sqlx/
git commit -m "Add issue_ownership view as the single ownership definition"
```

---

### Task 2: Capture the behaviour-neutrality baseline

This must happen BEFORE any call site changes, or there is nothing to compare against.

**Files:**
- Create: `/tmp/ownership-baseline.txt` (not committed)

- [ ] **Step 1: Copy the live catalog**

```bash
docker cp longbox:/data/longbox.db /tmp/ownership-base.db
docker cp longbox:/data/longbox.db-wal /tmp/ownership-base.db-wal
```

- [ ] **Step 2: Record the three numbers the spec names**

```bash
sqlite3 /tmp/ownership-base.db "
SELECT 'missing_total', COUNT(*) FROM issues i
  WHERE NOT EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id
                    AND f.status='owned' AND f.is_present=1)
UNION ALL
SELECT 'owned_files', COUNT(*) FROM files WHERE status='owned' AND is_present=1
UNION ALL
SELECT 'series_with_missing', COUNT(DISTINCT i.series_id) FROM issues i
  WHERE NOT EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id
                    AND f.status='owned' AND f.is_present=1);
" | tee /tmp/ownership-baseline.txt
```

Expected: three rows. Record them; Task 9 re-runs this and requires identical output.

- [ ] **Step 3: Record the live dashboard payload**

```bash
curl -s http://localhost:3000/api/stats > /tmp/ownership-stats-before.json
cat /tmp/ownership-stats-before.json
```

- [ ] **Step 4: No commit** — these are scratch artefacts, deliberately not committed.

---

### Task 3: Convert `longbox-db/src/issue_repo.rs` (2 sites)

**Files:**
- Modify: `longbox-db/src/issue_repo.rs:147`, `:186`

- [ ] **Step 1: Convert both sites**

At line 147 (`list_shipped_unowned`-style query) and line 186 (`list_pull_candidates`), replace this exact block:

```sql
             AND NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1
             )
```

with:

```sql
             AND NOT EXISTS (
               SELECT 1 FROM issue_ownership o
               WHERE o.issue_id = i.id AND o.is_owned = 1
             )
```

Leave every other clause — the `cover_date` filters, the `pull_attempts` exclusion — untouched.

- [ ] **Step 2: Regenerate the cache and build**

Run: `./scripts/prepare-sqlx.sh && SQLX_OFFLINE=true cargo build -p longbox-db`
Expected: no errors.

- [ ] **Step 3: Run the existing suite**

Run: `SQLX_OFFLINE=true cargo test --workspace`
Expected: all pass. `list_pull_candidates` has existing coverage in `longbox-pull/tests/sweep.rs`; a failure here means the conversion changed meaning.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
git add longbox-db/src/issue_repo.rs .sqlx/
git commit -m "Use issue_ownership in issue_repo"
```

---

### Task 4: Convert `longbox-web/src/routes/missing.rs` (4 sites)

**Files:**
- Modify: `longbox-web/src/routes/missing.rs:93`, `:121`, `:149`, `:175`

- [ ] **Step 1: Convert all four sites**

Each is the same block inside a `query_as!(MissingRow, …)`. Replace:

```sql
                     AND NOT EXISTS (
                       SELECT 1 FROM files f
                       WHERE f.issue_id = i.id
                         AND f.status = 'owned'
                         AND f.is_present = 1
                     )
```

with:

```sql
                     AND NOT EXISTS (
                       SELECT 1 FROM issue_ownership o
                       WHERE o.issue_id = i.id AND o.is_owned = 1
                     )
```

Two of the four are indented one level less (`WHERE NOT EXISTS (` at :149 and :175 rather than `AND NOT EXISTS (`). Match the surrounding indentation; do not change the `WHERE`/`AND` keyword.

- [ ] **Step 2: Regenerate and build**

Run: `./scripts/prepare-sqlx.sh && SQLX_OFFLINE=true cargo build -p longbox-web`
Expected: no errors.

- [ ] **Step 3: Run the suite**

Run: `SQLX_OFFLINE=true cargo test --workspace`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
git add longbox-web/src/routes/missing.rs .sqlx/
git commit -m "Use issue_ownership in the missing routes"
```

---

### Task 5: Convert `stats.rs`, `pull.rs`, `creator_repo.rs`, `pull_attempt_repo.rs` (6 sites)

**Files:**
- Modify: `longbox-web/src/routes/stats.rs:78`, `:85`
- Modify: `longbox-web/src/routes/pull.rs:211`
- Modify: `longbox-db/src/creator_repo.rs:51`
- Modify: `longbox-db/src/pull_attempt_repo.rs:297`, `:324`

- [ ] **Step 1: Convert the four `NOT EXISTS` sites**

`stats.rs:78`, `stats.rs:85`, `pull.rs:211` use `f.issue_id = i.id`. Replace the inner block with:

```sql
               SELECT 1 FROM issue_ownership o
               WHERE o.issue_id = i.id AND o.is_owned = 1
```

`pull_attempt_repo.rs:297` and `:324` correlate on `pull_attempts.issue_id`, not `i.id`. Replace with:

```sql
               SELECT 1 FROM issue_ownership o
               WHERE o.issue_id = pull_attempts.issue_id AND o.is_owned = 1
```

Getting this correlation wrong is the most likely mistake in the whole plan — it would silently make the stale-grabbed purge match every row.

- [ ] **Step 2: Convert the one positive `EXISTS` site**

`creator_repo.rs:51` is the only positive form. Replace:

```sql
             AND EXISTS (SELECT 1 FROM files f
                         WHERE f.issue_id = i.id AND f.status = 'owned' AND f.is_present = 1)
```

with:

```sql
             AND EXISTS (SELECT 1 FROM issue_ownership o
                         WHERE o.issue_id = i.id AND o.is_owned = 1)
```

Note this is `EXISTS`, not `NOT EXISTS` — do not invert it.

- [ ] **Step 3: Regenerate, build, test**

Run: `./scripts/prepare-sqlx.sh && SQLX_OFFLINE=true cargo test --workspace`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
git add longbox-web/src/routes/stats.rs longbox-web/src/routes/pull.rs \
        longbox-db/src/creator_repo.rs longbox-db/src/pull_attempt_repo.rs .sqlx/
git commit -m "Use issue_ownership in stats, pull, creator and attempt repos"
```

---

### Task 6: Convert the four `series_repo.rs` missing-count subqueries

**Files:**
- Modify: `longbox-db/src/series_repo.rs:467`, `:475`, `:554`, `:562`

These sit inside `COUNT(DISTINCT CASE WHEN NOT EXISTS (…) THEN i.id END)`. **Only the `NOT EXISTS` subquery changes.** The surrounding `COUNT(DISTINCT CASE …)` and the sibling per-status counters at `:452` and `:539` must not be touched.

- [ ] **Step 1: Convert the four subqueries**

Replace each:

```sql
                 SELECT 1 FROM files f2
                 WHERE f2.issue_id = i.id
                   AND f2.status = 'owned'
                   AND f2.is_present = 1
```

with:

```sql
                 SELECT 1 FROM issue_ownership o
                 WHERE o.issue_id = i.id AND o.is_owned = 1
```

- [ ] **Step 2: Confirm the per-status counters are untouched**

Run: `grep -n "WHEN f.status = 'owned' AND f.is_present = 1" longbox-db/src/series_repo.rs`
Expected: exactly 2 hits (lines ~452 and ~539). If fewer, a breakdown counter was converted by mistake — revert it.

- [ ] **Step 3: Regenerate, build, test**

Run: `./scripts/prepare-sqlx.sh && SQLX_OFFLINE=true cargo test --workspace`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
git add longbox-db/src/series_repo.rs .sqlx/
git commit -m "Use issue_ownership in series missing counts"
```

---

### Task 7: Comment the deliberately-unconverted sites

Without this, the next reader "finishes the job" and changes behaviour.

**Files:**
- Modify: `longbox-db/src/series_repo.rs` (above lines ~452, ~732, ~820, ~958)
- Modify: `longbox-web/src/integrity_scan.rs` (above line ~547)

- [ ] **Step 1: Add the four `series_repo.rs` comments**

Above the per-status breakdown (~452, and the same shape at ~539):

```rust
    // NOT the ownership predicate: this is a per-STATUS breakdown, and
    // `owned_count` sits beside needs_review/ignored/unmatched. The
    // `issue_ownership` view answers a boolean "is it owned"; it cannot
    // express this. Deliberately hand-written.
```

Above the Library Tidy check (~732, same shape at ~853):

```rust
    // NOT the ownership predicate: `status IN ('owned','needs_review',
    // 'unmatched')` asks "does this series have ANY file on disk", for
    // consecutive_empty_scans / auto_tidy_due_at. Folding it into
    // `issue_ownership` would silently change auto-tidy behaviour.
```

Above `last_matched_count` (~820):

```rust
    // NOT the ownership predicate: counts owned FILES, not owned
    // ISSUES. Different cardinality -- two files on one issue count
    // twice here and once in `issue_ownership`.
```

Above the enrichment `LEFT JOIN` (~958):

```rust
    // NOT the ownership predicate: a per-series aggregate, not a
    // per-issue boolean.
```

- [ ] **Step 2: Add the `integrity_scan.rs` comment**

Above the orphan query (~547):

```rust
    // NOT the ownership predicate: `issue_id IS NULL AND status =
    // 'owned'` detects BROKEN rows -- owned, pointing at nothing. The
    // `issue_ownership` view is keyed by issue and cannot see these.
```

- [ ] **Step 3: Build and commit**

```bash
cargo fmt --all
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
SQLX_OFFLINE=true cargo test --workspace
git add longbox-db/src/series_repo.rs longbox-web/src/integrity_scan.rs
git commit -m "Say why each unconverted ownership predicate stays hand-written"
```

---

### Task 8: Mutation-verify that every converted site actually uses the view

A site that still passes when the view is broken is not using it — the silent-partial-conversion failure this whole PR exists to prevent.

- [ ] **Step 1: Break the view**

Edit `longbox-db/migrations/20260805000000_add_issue_ownership_view.sql`, changing `AND f.is_present = 1` to `AND f.is_present = 0`, then:

Run: `./scripts/prepare-sqlx.sh`

- [ ] **Step 2: Confirm the build compiled and the suite RAN**

Run: `SQLX_OFFLINE=true cargo build --workspace --tests`
Expected: no errors. A build failure invalidates the mutation — fix it before reading any result.

- [ ] **Step 3: Run the suite and record what fails**

Run: `SQLX_OFFLINE=true cargo test --workspace 2>&1 | grep -E "^test result|FAILED"`
Expected: `issue_ownership_view` FAILS, and at least one test per converted area fails (pull sweep, missing routes, stats).

Write the failing list into the PR body. If a converted area has **no** failing test, that area has no coverage of the ownership path — note it explicitly rather than assuming it is fine.

- [ ] **Step 4: Revert the mutation**

Restore `is_present = 1`, then run `./scripts/prepare-sqlx.sh` and confirm `SQLX_OFFLINE=true cargo test --workspace` is green again.

- [ ] **Step 5: No commit** — the mutation is never committed.

---

### Task 9: Prove the change did nothing

- [ ] **Step 1: Add the view to the baseline copy and re-count through it**

`/tmp/ownership-base.db` was copied before the migration existed, so it has
no view. Add it with the exact DDL from Task 1, then count both ways in one
statement so any disagreement is visible side by side:

```bash
sqlite3 /tmp/ownership-base.db "
CREATE VIEW IF NOT EXISTS issue_ownership AS
SELECT i.id AS issue_id, i.series_id AS series_id,
       EXISTS (SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1) AS is_owned
FROM issues i;

SELECT
  (SELECT COUNT(*) FROM issues i
     WHERE NOT EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id
                       AND f.status='owned' AND f.is_present=1)) AS hand_written,
  (SELECT COUNT(*) FROM issue_ownership WHERE is_owned = 0) AS via_view;
"
```

Expected: the two columns are equal, and equal to `missing_total` in
`/tmp/ownership-baseline.txt`. Any difference is a conversion bug, not an
improvement.

- [ ] **Step 2: Full gates**

```bash
cargo fmt --all --check
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
SQLX_OFFLINE=true cargo test --workspace
```

Expected: fmt clean, 0 clippy issues, 0 test failures.

- [ ] **Step 3: Open the PR**

Body must state: this changes no behaviour; the numbers that prove it (`missing_total` before/after); the mutation results from Task 8 including any area with no failing test; and the list of deliberately-unconverted sites with reasons.

```bash
git push -u origin fix/ownership-predicate-view
gh pr create --base main --head fix/ownership-predicate-view \
  --title "Consolidate the issue-ownership predicate behind a view" \
  --body-file /tmp/ownership-pr-body.md
```

Create the branch at the START of Task 1 with
`git checkout -b fix/ownership-predicate-view`, so every task's commit lands
on it.

- [ ] **Step 4: Do not merge without review.**

---

## Deliberate deviation from the spec

The spec called for a **per-site equivalence harness** — for each of the 12
converted sites, assert the view form and the hand-written form agree, then
delete the hand-written one. This plan does that **once** (Task 1) rather than
twelve times, and gets the per-site guarantee from Task 8's mutation instead.

Reason: twelve throwaway tests that are deleted immediately is a lot of
scaffolding for the same assurance. Breaking the view and requiring every
converted area to fail proves each site actually routes through it — which is
the property the per-site harness was reaching for. If the reviewer prefers
the literal per-site harness, Task 8 is where to expand it.

## Open question for the reviewer

Task 8 may reveal converted areas with **no test coverage of the ownership path** — `creator_repo`'s credits-fetch candidate query and `pull_attempt_repo`'s stale-grabbed purge are the likely candidates. If so, the choice is to add coverage in this PR or record the gap. Adding it is preferable; recording it silently is not.
