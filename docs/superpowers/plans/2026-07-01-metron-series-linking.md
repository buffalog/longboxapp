# Metron Series Linking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Populate `series.metron_id` on the owned catalog by matching each CV-linked series to Metron via its ComicVine id — activating the dormant series-finished-status feature (#7/#10) and laying the foundation for later Metron per-issue work (credits/MetronInfo).

**Architecture:** A continuous low-priority background resolver drains a work-list of CV-linked-but-not-yet-Metron-checked series, calls Metron `series/?cv_id=<cvid>` (the existing `fetch_series_by_cv_id`), and records the outcome: matched → store `metron_id`; no Metron match → mark checked so it isn't re-queried (the no-churn lesson from the credits resolver). Throttled by Metron's existing rate limiter. Series-only — issue linking and credits are out of scope.

**Tech Stack:** Rust, tokio, sqlx (SQLite, offline metadata), the existing `longbox-metron` client + `governor` rate limiter.

**Locked kickoff decisions:**
1. **Series-only** (no issue-linking, no credits — those are later follow-ups).
2. Add a `series.metron_link_checked_at` column so unmatched series aren't re-queried forever.
3. Link **all** ~694 CV-linked series (not owned-only).
4. **Continuous background resolver** (mirror `credits_resolver`), spawned at bootstrap, gated on `metron.is_some()`.
5. **Deterministic matching only** — series by exact `cv_id`; no fuzzy title matching.
6. Keep the #7 finished-enrichment **separate** — linking only populates `metron_id`; the existing `POST /api/series/enrich-finished` (manual) consumes it. *(See "Open toggle" below.)*
7. Credits explicitly out of scope.

**Open toggle (decide before/at review):** `enrich-finished` is **manual-only** — no scheduler runs it. So with #6 kept separate, series-status refreshes only when that endpoint is POSTed. If you'd rather the linker refresh status automatically, it can call `fetch_series_detail` + `set_finished` inline right after linking each series (+1 Metron call/series, still ~1,400 total, within budget). This plan builds **linking-only** per decision #6; the inline-status variant is a small add noted at Task 6.

**CI is enforced:** every commit must pass `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. **Run `cargo fmt` before each commit.** Use `SQLX_OFFLINE=true` for cargo commands.

**Pre-flight:**
```bash
git checkout -b feat/metron-series-linking
SQLX_OFFLINE=true cargo test --workspace   # baseline green
export DATABASE_URL="sqlite:/tmp/lb-metronlink-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
```

**Reviewer gate** (superpowers:code-reviewer) after **Commit 3** (the resolver — Metron budget + no-churn correctness).

---

## Investigation facts (verified — don't re-derive)

- **`MetronClient::fetch_series_by_cv_id(cv_id: i64) -> Result<Option<MetronSeriesRef>, MetronError>`** exists (`longbox-metron/src/client.rs:196`). Returns `Some` with `MetronSeriesRef.metron_series_id: i64` on match, `Ok(None)` when Metron has no series for that cv_id (empty list — NOT an error). Confirm the exact `MetronSeriesRef` field name in `longbox-metron/src/projection.rs`.
- **`series_repo::set_metron_id`** exists (`series_repo.rs:219`) but is race-guarded and doesn't touch the new checked column — this plan adds a dedicated `mark_metron_link_checked` writer instead.
- **`series.metron_id`** is `Option<String>` (TEXT) — stringify the `i64`.
- **`AppState.metron: Option<Arc<longbox_metron::MetronClient>>`** (`longbox-web/src/state.rs:23`) — cloneable Arc, hands to a `tokio::spawn`ed task.
- **Bootstrap spawn site:** `longbox-web/src/bootstrap.rs` (next to `longbox_cv_enrichment::spawn` + `spawn_credits_resolver`, ~line 176-184; Metron client built ~192-198). `spawn_wal_checkpoint` (bootstrap.rs) is the fire-and-forget template.
- **Resolver template:** `longbox-cv-enrichment/src/credits_resolver.rs` (`spawn_*` + loop: batch work-list → idle-sleep-if-empty → per-item call + persist → warn-and-continue). Metron's own rate limiter throttles, so no extra spacing wrapper is needed.
- **Dormant consumer this activates:** `series_repo::list_metron_linked_unfinished` (`series_repo.rs:260`, `WHERE metron_id IS NOT NULL AND finished = 0`) — 0 rows today because nothing is linked; the `POST /api/series/enrich-finished` route (`longbox-web/src/routes/series.rs:63`) drains it.
- **`longbox-db` test harness:** integration tests in `longbox-db/tests/series.rs` using `common::fresh_pool()` + `series_repo::insert(NewSeries{..})`; `walking_dead()` fixture (cv_id 12345, metron_id None).

---

## File Structure

| File | Responsibility | Commit |
|------|----------------|--------|
| `longbox-db/migrations/<ts>_add_metron_link_checked.sql` (create) | `series.metron_link_checked_at` column | 1 |
| `longbox-db/src/series_repo.rs` (modify) | `list_metron_link_candidates` + `mark_metron_link_checked` | 2 |
| `longbox-db/tests/series.rs` (modify) | repo tests | 2 |
| `longbox-web/src/metron_link.rs` (create) | linking resolver `spawn_metron_linker` + loop | 3 |
| `longbox-web/src/lib.rs` or `main.rs` (modify) | `mod metron_link;` | 3 |
| `longbox-web/src/bootstrap.rs` (modify) | spawn the linker, gated on `metron.is_some()` | 3 |

---

## Commit 1 — Migration

### Task 1: `metron_link_checked_at` column

**Files:** Create `longbox-db/migrations/<timestamp>_add_metron_link_checked.sql`

- [ ] **Step 1: Create the migration.** First `ls longbox-db/migrations/ | tail -3` to find the latest filename; name the new file with a timestamp that sorts AFTER it (e.g. `20260701120000_add_metron_link_checked.sql` if the latest is `20260701000000_add_creator_credits.sql`). Content:

```sql
-- When the Metron-linking resolver last checked this series against Metron
-- (matched or not). NULL = never checked. Distinguishes "no Metron match"
-- from "not yet attempted" so the resolver's work-list doesn't re-query
-- unmatched series forever.
ALTER TABLE series ADD COLUMN metron_link_checked_at TIMESTAMP;
```

- [ ] **Step 2: Verify it applies + migration test.**

```bash
export DATABASE_URL="sqlite:/tmp/lb-metronlink-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
SQLX_OFFLINE=true cargo test -p longbox-db migration
```
Expected: applies cleanly; migration tests pass. (No `cargo sqlx prepare` yet — no query references the new column until Commit 2. `SeriesRow` selects explicit columns, so an added column doesn't break existing queries. If the `migration_creates_all_indexes`/`_tables` test hardcodes a column count for `series`, update it; otherwise nothing to change.)

- [ ] **Step 3: Commit**

```bash
git add longbox-db/migrations/
git commit -m "feat(db): add series.metron_link_checked_at"
```

---

## Commit 2 — DB layer

### Task 2: work-list + mark writer

**Files:** Modify `longbox-db/src/series_repo.rs`, `longbox-db/tests/series.rs`

- [ ] **Step 1: Write the failing tests** — add to `longbox-db/tests/series.rs`

```rust
#[tokio::test]
async fn metron_link_candidates_and_mark() {
    let pool = fresh_pool().await;
    // (a) CV-linked, unlinked, unchecked -> a candidate
    let cand = series_repo::insert(&pool, walking_dead()).await.unwrap(); // cv_id 12345
    // (b) no cv_id -> excluded
    series_repo::insert(&pool, NewSeries { cv_id: None, ..walking_dead() }).await.unwrap();
    // (c) already Metron-linked -> excluded
    let linked = series_repo::insert(&pool, NewSeries { cv_id: Some(222), ..walking_dead() }).await.unwrap();
    series_repo::mark_metron_link_checked(&pool, linked.id, Some("916")).await.unwrap();
    // (d) checked with NO match -> excluded (this is the no-churn case)
    let nomatch = series_repo::insert(&pool, NewSeries { cv_id: Some(333), ..walking_dead() }).await.unwrap();
    series_repo::mark_metron_link_checked(&pool, nomatch.id, None).await.unwrap();

    let cands = series_repo::list_metron_link_candidates(&pool, 50).await.unwrap();
    assert_eq!(cands, vec![(cand.id, 12345)], "only the cv-linked, unlinked, unchecked series");

    // mark the candidate as matched -> metron_id set, and it leaves the work-list
    series_repo::mark_metron_link_checked(&pool, cand.id, Some("916")).await.unwrap();
    let after = series_repo::list_metron_link_candidates(&pool, 50).await.unwrap();
    assert!(after.is_empty());
    let row = series_repo::find_by_id(&pool, cand.id).await.unwrap().unwrap();
    assert_eq!(row.metron_id.as_deref(), Some("916"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db metron_link_candidates_and_mark`
Expected: FAIL (undefined functions).

- [ ] **Step 3: Implement** — add to `longbox-db/src/series_repo.rs`

```rust
/// `(series_id, cv_id)` for CV-linked series not yet Metron-linked and not yet
/// checked. Drives the Metron-linking resolver; excludes already-linked and
/// already-checked-no-match series so the resolver converges.
pub async fn list_metron_link_candidates<'e, E>(executor: E, limit: i64) -> Result<Vec<(i64, i64)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", cv_id AS "cv_id!: i64"
           FROM series
           WHERE cv_id IS NOT NULL
             AND metron_id IS NULL
             AND metron_link_checked_at IS NULL
           ORDER BY id ASC
           LIMIT ?"#,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.cv_id)).collect())
}

/// Record a Metron-link check: stamp `metron_link_checked_at` (so the series
/// leaves the work-list) and, on a match, set `metron_id`. `COALESCE` protects
/// an already-set metron_id from being clobbered by a racing writer.
/// `metron_id = None` = checked, no Metron match.
pub async fn mark_metron_link_checked<'e, E>(
    executor: E,
    series_id: i64,
    metron_id: Option<&str>,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET metron_id = COALESCE(metron_id, ?),
               metron_link_checked_at = CURRENT_TIMESTAMP,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        metron_id,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
```

- [ ] **Step 4: sqlx prepare + run**

```bash
export DATABASE_URL="sqlite:/tmp/lb-metronlink-prepare.db?mode=rwc"
cargo sqlx prepare --workspace -- --all-targets
SQLX_OFFLINE=true cargo test -p longbox-db metron_link_candidates_and_mark
```
Expected: PASS; `git diff --stat .sqlx/` adds 2 query json files, no unrelated deletions.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add longbox-db/src/series_repo.rs longbox-db/tests/series.rs .sqlx
git commit -m "feat(db): metron link candidates work-list + mark writer"
```

---

## Commit 3 — Linking resolver + spawn  ⟶ REVIEWER GATE after this commit

### Task 3: the resolver

**Files:** Create `longbox-web/src/metron_link.rs`; modify `longbox-web/src/lib.rs` (or wherever modules are declared) + `longbox-web/src/bootstrap.rs`

- [ ] **Step 1: Create `longbox-web/src/metron_link.rs`**

```rust
//! Continuous Metron-linking resolver. Drains CV-linked series not yet checked
//! against Metron, matching each by its ComicVine id (`series/?cv_id=`), and
//! recording the outcome: matched -> `metron_id`; no match -> checked-only (so
//! it never re-queries). Fire-and-forget; Metron's own rate limiter throttles.
//! Populating `metron_id` activates the dormant series-finished enrichment.
use std::sync::Arc;
use std::time::Duration;

use longbox_db::{series_repo, Pool};
use longbox_metron::MetronClient;

/// Idle re-check interval when the work-list is empty.
const IDLE_SLEEP: Duration = Duration::from_secs(300);
/// Series per work-list batch.
const BATCH: i64 = 50;

/// Spawn the linker onto the tokio runtime. `metron` is the shared client.
pub fn spawn_metron_linker(db: Pool, metron: Arc<MetronClient>) {
    tokio::spawn(link_loop(db, metron));
}

async fn link_loop(db: Pool, metron: Arc<MetronClient>) {
    loop {
        let batch = match series_repo::list_metron_link_candidates(&db, BATCH).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "longbox_metron_link", error = %e, "candidate query failed");
                tokio::time::sleep(IDLE_SLEEP).await;
                continue;
            }
        };
        if batch.is_empty() {
            tokio::time::sleep(IDLE_SLEEP).await;
            continue;
        }
        let (mut linked, mut no_match) = (0usize, 0usize);
        for (series_id, cv_id) in &batch {
            match metron.fetch_series_by_cv_id(*cv_id).await {
                Ok(Some(sref)) => {
                    let mid = sref.metron_series_id.to_string();
                    match series_repo::mark_metron_link_checked(&db, *series_id, Some(&mid)).await {
                        Ok(_) => linked += 1,
                        Err(e) => tracing::warn!(target: "longbox_metron_link",
                            series_id, error = %e, "mark link failed"),
                    }
                }
                // Metron has no series for this CV id — mark checked (no match)
                // so it drops out of the work-list; do not churn.
                Ok(None) => {
                    let _ = series_repo::mark_metron_link_checked(&db, *series_id, None).await;
                    no_match += 1;
                }
                // Transient (rate-limit / network / http) — leave unchecked, retry.
                Err(e) => tracing::warn!(target: "longbox_metron_link",
                    cv_id, error = %e, "metron series fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_metron_link", linked, no_match, "metron link pass");
    }
}
```

> Verify the `MetronSeriesRef` field name (`metron_series_id`) against `longbox-metron/src/projection.rs` — if it's named differently (e.g. `id`), use the real name. Confirm `fetch_series_by_cv_id`'s exact return type.

- [ ] **Step 2: Declare the module** — add `pub mod metron_link;` (or `mod metron_link;`) alongside the other module declarations in `longbox-web/src/lib.rs` (or `main.rs` — match where `bootstrap`/`state` are declared).

- [ ] **Step 3: Spawn at bootstrap** — in `longbox-web/src/bootstrap.rs`, after the Metron client is built and stored on `AppState` (~line 192-217), and gated on the Metron client existing, add:

```rust
    if let Some(metron) = state.metron.clone() {
        crate::metron_link::spawn_metron_linker(db.clone(), metron);
    }
```

Place it near the other background spawns (`spawn_credits_resolver`, `spawn_wal_checkpoint`). Confirm `state.metron` is `Option<Arc<MetronClient>>` and `db` (the `Pool`) is in scope at that point; if the spawn must happen before `state` is fully built, use whatever local holds the `Option<Arc<MetronClient>>` (the client built at bootstrap.rs:192-198) and `db.clone()`.

- [ ] **Step 4: Build + test the workspace**

```bash
SQLX_OFFLINE=true cargo build --workspace
SQLX_OFFLINE=true cargo test --workspace
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean compile, all green, clippy clean. (The resolver loop is HTTP-bound; its correctness rests on the Task 2 work-list + mark tests — the loop is thin glue.)

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add longbox-web/src/metron_link.rs longbox-web/src/lib.rs longbox-web/src/bootstrap.rs
git commit -m "feat(web): continuous Metron series-linking resolver + spawn"
```

- [ ] **Step 6: REVIEWER GATE** — run `superpowers:code-reviewer` over Commits 1–3. Focus: the resolver converges (matched → linked; no-match → checked-only, never re-queried; transient error → retried, NOT marked); it can't exceed the Metron budget (rides the shared rate limiter, one call/series); the `COALESCE` guard; spawn is gated on `metron.is_some()`. Address findings before wrapping up.

---

## Final verification

- [ ] `SQLX_OFFLINE=true cargo test --workspace` — green.
- [ ] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check` — clean (CI gates).
- [ ] **Live smoke (post-deploy):** rebuild + `up -d`, watch `tracing` target `longbox_metron_link` log "metron link pass" with a nonzero `linked`; confirm `SELECT COUNT(*) FROM series WHERE metron_id IS NOT NULL` climbs from 0. Then (per decision #6) `POST /api/series/enrich-finished` and confirm it now has a non-empty work-list (finished-status starts populating) — where before it reported everything "skipped".

## Out of scope
- Issue-linking (`metron_issue_id`) — deferred; add when a per-issue consumer (credits/MetronInfo) is built.
- Metron credits.
- Auto-triggering `enrich-finished` (unless you take the "Open toggle" inline-status variant).
- Re-checking already-checked series (no staleness re-scan in v1).

## Self-review notes
- **Decision coverage:** series-only (no issue/credit code) ✓; checked column (Task 1) + no-churn work-list (Task 2) ✓; all CV-linked (`WHERE cv_id IS NOT NULL`, no owned filter) ✓; background resolver + gated spawn (Task 3) ✓; deterministic cv_id match (`fetch_series_by_cv_id`, no fuzzy) ✓; #7 kept separate (linker only writes metron_id; enrich-finished unchanged) ✓; no credits ✓.
- **Type consistency:** `list_metron_link_candidates -> Vec<(i64,i64)>` (series_id, cv_id) → resolver iterates `(series_id, cv_id)`. `mark_metron_link_checked(_, series_id, Option<&str>)` — resolver passes `Some(&metron_series_id.to_string())` on match, `None` on no-match. `fetch_series_by_cv_id -> Result<Option<MetronSeriesRef>>` → `Ok(Some)`/`Ok(None)`/`Err` arms. `MetronSeriesRef.metron_series_id: i64` stringified for the TEXT `metron_id` column.
- **No new client method needed** (series-only uses the existing `fetch_series_by_cv_id`) — the issue-list method the earlier map flagged is only for the deferred issue-linking.
