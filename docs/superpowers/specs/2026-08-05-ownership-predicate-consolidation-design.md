# Consolidate the issue-ownership predicate behind a view

**Date:** 2026-08-05
**Status:** approved, not yet implemented
**Sequence:** PR 1 of 2. PR 2 (collected-edition / `covered` state) depends on this.

## Problem

"Is this issue owned?" is expressed as a hand-written SQL `NOT EXISTS` block in
roughly 15 places, across 12 files in 4 crates:

```sql
NOT EXISTS (
    SELECT 1 FROM files f
    WHERE f.issue_id = i.id
      AND f.status = 'owned'
      AND f.is_present = 1
)
```

Measured: 38 live occurrences of the ownership predicate in `.rs` files
(excluding tests and comments), ~15 of them the `NOT EXISTS` "missing" form.

Nothing is broken today — the copies that answer this question agree. The
problem is prospective and specific: **PR 2 adds a third ownership state
(`covered`), and adding it means editing every copy correctly.** Editing all N
copies correctly is precisely what has failed twice in the last week in this
codebase:

- three separator normalisers, only one of which knew about `_`
- two digest-freshness rules, only one of which validated against disk

Both shipped as silent, invisible failures. This PR removes the class before
PR 2 walks into it.

## Non-goals

This PR buys **nothing user-visible**. It changes no behaviour, fixes no bug,
and must not. Its entire value is that PR 2 becomes a one-line change to a view
definition instead of 15 correct edits.

## Approach

Add a database view. Call sites join it instead of restating the predicate.

```sql
CREATE VIEW issue_ownership AS
SELECT i.id        AS issue_id,
       i.series_id AS series_id,
       EXISTS (SELECT 1 FROM files f
               WHERE f.issue_id = i.id
                 AND f.status = 'owned'
                 AND f.is_present = 1) AS is_owned
FROM issues i;
```

### Why a view, and not the alternatives

**Rejected — shared SQL fragment constant.** A `const OWNED: &str` interpolated
into queries cannot work with sqlx's `query!` macro, which requires literal SQL
at compile time. Every call site would drop to runtime `query()`, losing the
compile-time checking the workspace depends on and that CI gates on via the
committed `.sqlx/` offline cache. Trading type safety for deduplication is the
wrong direction.

**Rejected — repo functions.** `issue_repo::owned_issue_ids()` and friends.
Most call sites need ownership *inside* a larger query — joins, aggregates,
`list_pull_candidates`' candidate filter. Extracting it forces either N+1
round-trips or passing large `IN (...)` lists.

**Chosen — view.** One definition, living in a migration beside the tables it
describes. Compile-checked sqlx keeps working because a view is schema. The
wrong value becomes *unavailable* rather than merely discouraged.

### Spike results (both risks retired before committing to this design)

Run against a copy of the live catalog, 2026-08-05:

| Check | Result |
|---|---|
| `query!` compile-checks against the view | yes, `.sqlx` entry generated |
| `SQLX_OFFLINE=true` build (the CI condition) | clean |
| Query plan, hand-written vs view | **identical** — both `SCAN i USING COVERING INDEX idx_issues_series` + `SEARCH f USING COVERING INDEX idx_files_issue_status_present` |
| Timing, 20 runs each | **1.07 ms both** |
| Result agreement | **401 / 401** |

SQLite inlines the view completely, so the dedicated indexes added by
`20260608000000_add_dashboard_stats_indexes.sql` keep being used.

## Scope

### Converted (~15 sites)

Every site asking "is this issue owned?":

Counted per file (occurrences of `status = 'owned'`; not all are the
`NOT EXISTS` form, so the implementation plan must classify each one before
converting it):

| file | occurrences | notes |
|---|---|---|
| `longbox-db/src/series_repo.rs` | 13 | ownership counts; **2 are the Library Tidy emptiness check — excluded, see below** |
| `longbox-web/src/routes/missing.rs` | 4 | the missing lists |
| `longbox-web/src/routes/stats.rs` | 4 | dashboard counts |
| `longbox-db/src/issue_repo.rs` | 3 | includes `list_pull_candidates` |
| `longbox-web/src/routes/pull.rs` | 1 | `search_all_missing` |
| `longbox-web/src/integrity_scan.rs` | 2 | **both are the orphan predicate — excluded, see below** |

`longbox-db/src/creator_repo.rs`, `opds_repo.rs`, `file_repo.rs`,
`pull_attempt_repo.rs` and `longbox-postprocess/src/processor.rs` also contain
the string; the plan must classify those before assuming they convert.

**Decision: convert all of them, not only those PR 2 needs.** A partial
conversion leaves copies that will drift, which is the exact failure this PR
exists to prevent. Cost: the PR touches four crates.

### Deliberately NOT converted

Each exclusion gets a comment at the site saying why, so a later reader does
not "finish the job" and change behaviour.

1. **Library Tidy's emptiness check** — `series_repo.rs:732` and `:853` use
   `status IN ('owned', 'needs_review', 'unmatched')`. This is a **different
   question**: "does this series have *any* file on disk", used for
   `consecutive_empty_scans` and `auto_tidy_due_at`. It is not drift. Folding it
   into `issue_ownership` would silently change auto-tidy behaviour.

2. **`integrity_scan` entirely** — verified 2026-08-05: both of its
   `status = 'owned'` occurrences are the orphan predicate
   (`issue_id IS NULL AND status = 'owned'`, line 548 and its doc comment at
   317), which detects *broken* rows. It has **no** ownership `NOT EXISTS` to
   convert. An earlier draft of this spec listed it as a conversion target;
   that was wrong.

3. **Migration SQL** — historical. Must never be rewritten.

## Data model note

`is_owned` is SQLite integer `0`/`1`, and sqlx types it `i64`. Call sites
compare `is_owned = 1` explicitly rather than casting to `BOOLEAN`, so the
storage type stays visible at the point of use.

## Testing

The risk is not "does the view work" — the spike settled that. The risk is
**a call site changing meaning during conversion**.

1. **Equivalence harness (temporary).** For each converted site, assert the view
   form and the hand-written form return identical results against the same
   fixture. Then delete the hand-written form. These are scaffolding, removed
   when the conversion lands; they are not permanent tests.

2. **Mutation check (the one that matters).** Break the view — `is_present = 1`
   → `is_present = 0` — and confirm **every** converted call site's test fails.
   A site whose test still passes is not actually using the view, which is the
   silent-partial-conversion failure mode. Confirm the build compiled and the
   suite ran before reading any mutation result.

3. **Full workspace suite green**, plus `cargo fmt --check` and
   `clippy -D warnings`, per the standing pre-commit routine.

4. **`.sqlx/` regenerated** via `scripts/prepare-sqlx.sh` and committed — the
   repo rule for any commit touching SQL.

## Verification that it did nothing

Because the PR is behaviour-neutral by design, the proof is that observable
numbers do not move. Before and after, against the same catalog copy:

- total missing count (currently 401 on the measurement copy, 403 live)
- dashboard stats payload
- `list_pull_candidates` row count for a fixed series

Any difference is a bug in the conversion, not an improvement.

## Follow-on

PR 2 — collected-edition support. Adds a `covered` state so an issue whose
content exists only inside a grabbed trade paperback stops being reported as
missing and stops being searched every sweep. With this PR landed, that becomes
a change to the view definition plus the new grab path, rather than 15 edits.
