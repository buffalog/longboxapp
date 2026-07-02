# Metron Issue Linking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Populate `issues.metron_issue_id` for issues in Metron-linked series, by bulk-fetching each linked series' Metron issues and matching by issue number — the foundation for later Metron per-issue work (credits / MetronInfo). Standalone; credits is the next feature.

**Architecture:** A separate continuous background resolver (co-located with the series-linker, but its own loop) drains series that have a `metron_id` but whose issues haven't been linked yet, calls a new `fetch_issues_by_series_id`, matches our `issue.number` to Metron's via `IssueNumber::matches` (deterministic), and writes `metron_issue_id`. A per-series `metron_issues_linked_at` marker prevents re-fetching (the no-churn lesson from `metron_link_checked_at`). Does not touch the completed series-linker.

**Tech Stack:** Rust, tokio, sqlx (SQLite, offline metadata), `longbox-metron` client + rate limiter, `longbox_core::issue::IssueNumber`.

**Locked kickoff decisions:**
1. **Standalone** — issue-linking only; Metron credits is a separate later feature (its creator name-dedup needs its own investigation).
2. **Separate resolver** keyed on `metron_id IS NOT NULL AND metron_issues_linked_at IS NULL`; a new `series.metron_issues_linked_at` marker. Do NOT modify the finished series-linker.
3. **All issues** in Metron-linked series (bulk fetch is per-series regardless of ownership; non-owned issues getting Metron ids is free future value).
4. **Deterministic matching** — `IssueNumber::matches`; unmatched issues stay NULL; duplicate Metron numbers → first match.
5. **Paginate** the Metron issue list the same way `fetch_issues_by_store_date_range` does.
6. Credits out of scope.

**CI is enforced:** every commit must pass `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. **Run `cargo fmt` before each commit.** Use `SQLX_OFFLINE=true`.

**Pre-flight:**
```bash
git checkout -b feat/metron-issue-linking
SQLX_OFFLINE=true cargo test --workspace   # baseline green
export DATABASE_URL="sqlite:/tmp/lb-issuelink-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
```

**Reviewer gate** (superpowers:code-reviewer) after **Commit 4** (the resolver — Metron budget, no-churn, number-match correctness).

---

## Investigation facts (verified — don't re-derive)

- **Series-linking is deployed & complete**: 694/694 series checked, 606 have `metron_id`. This resolver targets those 606.
- **Metron bulk issue list works**: `GET /api/issue/?series_id=<metron id>` returns all of a series' issues, paginated (`MetronList { count, next, results }`; `PAGE_LIMIT = 500` so most series fit one page). Row `MetronIssueListRow { id, number, .. }` — `id` is the Metron issue id, `number` is what we match.
- **Clone target**: `MetronClient::fetch_issues_by_store_date_range` (`longbox-metron/src/client.rs:120-158`) — the exact pagination loop (`build_url("issue/", &[params])` → `execute_with_retry` → `parse_json::<MetronList<MetronIssueListRow>>` → project → break on `!next` or `out.len() >= count`).
- **Projection to mirror**: `MetronSeriesRef` + `project_series_ref` (`projection.rs:71,139`). Add a parallel `MetronIssueRef` + `project_issue_ref`.
- **DB writer to mirror**: `series_repo::set_metron_id` (`series_repo.rs:219`, race-guarded `WHERE id=? AND (col IS NULL OR col = ?)`). Add `issue_repo::set_metron_issue_id` the same shape. `issues.metron_issue_id` is `Option<String>` (TEXT) — stringify the `i64`.
- **`issue_repo::list_by_series(executor, series_id) -> Vec<IssueRow>`** exists (`issue_repo.rs:99`); `IssueRow` has `id: i64`, `number: String`, `metron_issue_id: Option<String>`.
- **`IssueNumber`**: `longbox_core::issue::IssueNumber`, `From<&str>`, `.matches(&other)` — natural comparison (zero-pad tolerant, suffix-distinguishing).
- **Resolver home + `is_terminal`**: put the new resolver in the existing `longbox-web/src/metron_link.rs` (its own `spawn_*` + loop); reuse the `is_terminal(&MetronError)` already defined there. Spawn at `bootstrap.rs` next to `spawn_metron_linker`, gated on `metron.is_some()`.
- **Latest migration**: `20260701120000_add_metron_link_checked.sql` — new file sorts after.

---

## File Structure

| File | Responsibility | Commit |
|------|----------------|--------|
| `longbox-db/migrations/20260701130000_add_metron_issues_linked.sql` (create) | `series.metron_issues_linked_at` column | 1 |
| `longbox-metron/src/models.rs` — (no change; `MetronIssueListRow` already exists) | — | 2 |
| `longbox-metron/src/projection.rs` (modify) | `MetronIssueRef` + `project_issue_ref` | 2 |
| `longbox-metron/src/client.rs` (modify) | `fetch_issues_by_series_id` | 2 |
| `longbox-metron/src/lib.rs` (modify) | re-export `MetronIssueRef` | 2 |
| `longbox-db/src/issue_repo.rs` (modify) | `set_metron_issue_id` | 3 |
| `longbox-db/src/series_repo.rs` (modify) | `list_metron_issue_link_candidates` + `mark_metron_issues_linked` | 3 |
| `longbox-db/tests/series.rs` (modify) | repo tests | 3 |
| `longbox-web/src/metron_link.rs` (modify) | `match_issue_links` (pure) + `spawn_metron_issue_linker` + loop | 4 |
| `longbox-web/src/bootstrap.rs` (modify) | spawn the issue-linker | 4 |

---

## Commit 1 — Migration

### Task 1: `metron_issues_linked_at` column

**Files:** Create `longbox-db/migrations/20260701130000_add_metron_issues_linked.sql`

- [ ] **Step 1: Create the migration** (confirm it sorts after `20260701120000` first)

```sql
-- When the Metron issue-linking resolver last fetched+matched this series'
-- Metron issues. NULL = not yet done. Only series with a metron_id are
-- candidates; this marks a linked series' issue-fetch as complete so it isn't
-- re-fetched (no-churn, mirrors metron_link_checked_at at the series level).
ALTER TABLE series ADD COLUMN metron_issues_linked_at TIMESTAMP;
```

- [ ] **Step 2: Verify** — `export DATABASE_URL=...` (pre-flight), `cargo sqlx migrate run --source longbox-db/migrations`, then `SQLX_OFFLINE=true cargo test -p longbox-db migration` (passes; the test checks table/index names, so no change needed). `SQLX_OFFLINE=true cargo build -p longbox-db` clean. No `sqlx prepare` yet.

- [ ] **Step 3: Commit**

```bash
git add longbox-db/migrations/
git commit -m "feat(db): add series.metron_issues_linked_at"
```

---

## Commit 2 — Metron client: fetch issues by series

### Task 2: `MetronIssueRef` + `project_issue_ref` + `fetch_issues_by_series_id`

**Files:** Modify `longbox-metron/src/projection.rs`, `longbox-metron/src/client.rs`, `longbox-metron/src/lib.rs`

- [ ] **Step 1: Write the failing projection test** — add to `longbox-metron/src/projection.rs` (a `#[cfg(test)] mod issue_ref_tests`)

```rust
#[cfg(test)]
mod issue_ref_tests {
    use super::*;
    use crate::models::{MetronEmbeddedSeriesLite, MetronIssueListRow};

    #[test]
    fn project_issue_ref_pulls_id_and_number() {
        let raw = MetronIssueListRow {
            id: 7997,
            series: MetronEmbeddedSeriesLite { name: "Saga".into(), volume: 1, year_began: 2012 },
            number: "1".into(),
            issue: None,
            cover_date: None,
            store_date: None,
            image: None,
            cover_hash: None,
            modified: None,
        };
        let r = project_issue_ref(raw);
        assert_eq!(r.metron_issue_id, 7997);
        assert_eq!(r.issue_number, "1");
    }
}
```

> Confirm `MetronEmbeddedSeriesLite`'s exact field names/types in `models.rs` and adjust the literal to match (the row shape is `models.rs:32-45`; the embedded series is `models.rs:51-55`).

- [ ] **Step 2: Run to verify failure** — `SQLX_OFFLINE=true cargo test -p longbox-metron project_issue_ref` → FAIL (undefined).

- [ ] **Step 3: Add the projection** in `longbox-metron/src/projection.rs` (near `MetronSeriesRef`)

```rust
/// One issue of a series from `GET /api/issue/?series_id=`. Just the fields
/// the issue-linking resolver needs: the Metron issue id and its number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetronIssueRef {
    pub metron_issue_id: i64,
    pub issue_number: String,
}

pub(crate) fn project_issue_ref(raw: crate::models::MetronIssueListRow) -> MetronIssueRef {
    MetronIssueRef {
        metron_issue_id: raw.id,
        issue_number: raw.number,
    }
}
```

- [ ] **Step 4: Add the client method** in `longbox-metron/src/client.rs` (inside `impl MetronClient`, after `fetch_issues_by_store_date_range`), cloning its pagination exactly — only the query params + projection differ. Ensure `project_issue_ref` and `MetronIssueRef` are imported (mirror how `project_series_ref`/`MetronSeriesRef` are imported at the top).

```rust
/// All of a series' issues from Metron (`issue/?series_id=`), paginated.
/// Returns id + number for each — the issue-linking resolver matches our
/// issue numbers against these.
#[instrument(target = "longbox_metron", skip(self))]
pub async fn fetch_issues_by_series_id(
    &self,
    metron_series_id: i64,
) -> Result<Vec<MetronIssueRef>, MetronError> {
    let mut out = Vec::new();
    let mut page: u32 = 1;
    let limit_str = PAGE_LIMIT.to_string();
    let series_str = metron_series_id.to_string();

    loop {
        let page_str = page.to_string();
        let url = self.build_url(
            "issue/",
            &[
                ("series_id", series_str.as_str()),
                ("page_size", limit_str.as_str()),
                ("page", page_str.as_str()),
            ],
        )?;
        let body = self.execute_with_retry(url).await?;
        let envelope: MetronList<MetronIssueListRow> = parse_json(&body)?;
        let total = envelope.count;
        let has_next = envelope.next.is_some();
        for row in envelope.results {
            out.push(project_issue_ref(row));
        }
        if !has_next || out.len() as i64 >= total {
            break;
        }
        page = page.saturating_add(1);
    }

    Ok(out)
}
```

- [ ] **Step 5: Re-export** in `longbox-metron/src/lib.rs` — add `MetronIssueRef` to the `pub use projection::{...}` line (next to `MetronSeriesRef`).

- [ ] **Step 6: Verify** — `SQLX_OFFLINE=true cargo test -p longbox-metron` (projection test passes) + `cargo build -p longbox-metron` + `cargo clippy -p longbox-metron --all-targets -- -D warnings` clean. (The HTTP method itself is exercised by the resolver + post-deploy smoke.)

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add longbox-metron/src/projection.rs longbox-metron/src/client.rs longbox-metron/src/lib.rs
git commit -m "feat(metron): fetch_issues_by_series_id + MetronIssueRef"
```

---

## Commit 3 — DB layer

### Task 3: writer + work-list + marker

**Files:** Modify `longbox-db/src/issue_repo.rs`, `longbox-db/src/series_repo.rs`, `longbox-db/tests/series.rs`

- [ ] **Step 1: Write the failing test** — add to `longbox-db/tests/series.rs`

```rust
#[tokio::test]
async fn metron_issue_link_candidates_writer_and_marker() {
    use longbox_db::{issue_repo, NewIssue};
    let pool = fresh_pool().await;
    // series with metron_id but issues not yet linked -> a candidate
    let s = series_repo::insert(&pool, walking_dead()).await.unwrap();
    series_repo::set_metron_id(&pool, s.id, "916").await.unwrap();
    let iid = issue_repo::insert(&pool, NewIssue {
        series_id: s.id, cv_issue_id: Some(1), metron_issue_id: None,
        number: "1".into(), title: None, cover_date: None, summary: None, cover_url: None,
    }).await.unwrap().id;
    // a series WITHOUT metron_id -> excluded
    series_repo::insert(&pool, NewSeries { cv_id: Some(9), ..walking_dead() }).await.unwrap();

    let cands = series_repo::list_metron_issue_link_candidates(&pool, 50).await.unwrap();
    assert_eq!(cands, vec![(s.id, "916".to_string())]);

    // link the issue + mark the series done
    issue_repo::set_metron_issue_id(&pool, iid, "7997").await.unwrap();
    series_repo::mark_metron_issues_linked(&pool, s.id).await.unwrap();

    // series leaves the work-list; issue carries the metron id
    assert!(series_repo::list_metron_issue_link_candidates(&pool, 50).await.unwrap().is_empty());
    let issue = issue_repo::find_by_id(&pool, iid).await.unwrap().unwrap();
    assert_eq!(issue.metron_issue_id.as_deref(), Some("7997"));
    // set race-guard: a second set with a different id does NOT clobber
    issue_repo::set_metron_issue_id(&pool, iid, "9999").await.unwrap();
    let issue2 = issue_repo::find_by_id(&pool, iid).await.unwrap().unwrap();
    assert_eq!(issue2.metron_issue_id.as_deref(), Some("7997"));
}
```

> Confirm `issue_repo::insert`, `find_by_id`, and `NewIssue` shapes against the real code before running (they're used by other tests in the workspace — mirror those). If `NewIssue` field names differ, match them.

- [ ] **Step 2: Run to verify failure** — `SQLX_OFFLINE=true cargo test -p longbox-db metron_issue_link_candidates_writer` → FAIL.

- [ ] **Step 3: Implement `set_metron_issue_id`** in `longbox-db/src/issue_repo.rs` (mirror `series_repo::set_metron_id`)

```rust
/// Link an issue to its Metron issue id. Race-guarded (only sets when NULL or
/// already equal), so a re-run or concurrent writer can't clobber.
pub async fn set_metron_issue_id<'e, E>(
    executor: E,
    issue_id: i64,
    metron_issue_id: &str,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE issues
           SET metron_issue_id = ?, updated_at = CURRENT_TIMESTAMP
           WHERE id = ?
             AND (metron_issue_id IS NULL OR metron_issue_id = ?)"#,
        metron_issue_id,
        issue_id,
        metron_issue_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
```

- [ ] **Step 4: Implement the work-list + marker** in `longbox-db/src/series_repo.rs`

```rust
/// `(series_id, metron_id)` for Metron-linked series whose issues haven't been
/// linked yet. Drives the issue-linking resolver.
pub async fn list_metron_issue_link_candidates<'e, E>(
    executor: E,
    limit: i64,
) -> Result<Vec<(i64, String)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", metron_id AS "metron_id!: String"
           FROM series
           WHERE metron_id IS NOT NULL
             AND metron_issues_linked_at IS NULL
           ORDER BY id ASC
           LIMIT ?"#,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.metron_id)).collect())
}

/// Stamp a series' issue-linking as done (matched, partially matched, or a
/// terminal fetch error) so it leaves the work-list.
pub async fn mark_metron_issues_linked<'e, E>(executor: E, series_id: i64) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET metron_issues_linked_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
```

- [ ] **Step 5: sqlx prepare + run**

```bash
export DATABASE_URL="sqlite:/tmp/lb-issuelink-prepare.db?mode=rwc"
cargo sqlx prepare --workspace -- --all-targets
SQLX_OFFLINE=true cargo test -p longbox-db metron_issue_link_candidates_writer
```
Expected: PASS; `.sqlx` adds 3 new query files (set_metron_issue_id, candidates, mark), no unrelated deletions.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add longbox-db/src/issue_repo.rs longbox-db/src/series_repo.rs longbox-db/tests/series.rs .sqlx
git commit -m "feat(db): metron issue-link writer + work-list + marker"
```

---

## Commit 4 — Issue-linking resolver  ⟶ REVIEWER GATE after this commit

### Task 4: pure matcher + resolver + spawn

**Files:** Modify `longbox-web/src/metron_link.rs`, `longbox-web/src/bootstrap.rs`

- [ ] **Step 1: Write the failing test for the pure matcher** — add to `longbox-web/src/metron_link.rs`'s `#[cfg(test)] mod tests`

```rust
#[test]
fn match_issue_links_by_number_deterministic() {
    use longbox_metron::MetronIssueRef;
    let metron = vec![
        MetronIssueRef { metron_issue_id: 100, issue_number: "1".into() },
        MetronIssueRef { metron_issue_id: 200, issue_number: "2".into() },
        MetronIssueRef { metron_issue_id: 300, issue_number: "2".into() }, // dup number
    ];
    // (issue_id, number, already_linked)
    let ours = vec![
        (11, "1".to_string(), false),    // -> 100
        (12, "002".to_string(), false),  // zero-pad tolerant -> 200 (first dup wins)
        (13, "5".to_string(), false),    // no Metron match -> skipped
        (14, "1".to_string(), true),     // already linked -> skipped
    ];
    let links = match_issue_links(&ours, &metron);
    assert_eq!(links, vec![(11, 100), (12, 200)]);
}
```

- [ ] **Step 2: Run to verify failure** — `SQLX_OFFLINE=true cargo test -p longbox-web match_issue_links` → FAIL.

- [ ] **Step 3: Add the pure matcher** in `longbox-web/src/metron_link.rs` (add imports `use longbox_core::issue::IssueNumber;` and `use longbox_metron::MetronIssueRef;`)

```rust
/// Match our issues to Metron issue refs by issue number (deterministic,
/// zero-pad tolerant via `IssueNumber::matches`). Input rows are
/// `(issue_id, number, already_linked)`. Skips already-linked issues and
/// unmatched numbers; on duplicate Metron numbers, first wins. Returns
/// `(issue_id, metron_issue_id)` to persist.
fn match_issue_links(ours: &[(i64, String, bool)], metron: &[MetronIssueRef]) -> Vec<(i64, i64)> {
    ours.iter()
        .filter(|(_, _, already_linked)| !*already_linked)
        .filter_map(|(id, number, _)| {
            let n = IssueNumber::from(number.as_str());
            metron
                .iter()
                .find(|r| n.matches(&IssueNumber::from(r.issue_number.as_str())))
                .map(|r| (*id, r.metron_issue_id))
        })
        .collect()
}
```

- [ ] **Step 4: Run to verify pass** — `SQLX_OFFLINE=true cargo test -p longbox-web match_issue_links` → PASS.

- [ ] **Step 5: Add the resolver** in `longbox-web/src/metron_link.rs` (reuse the existing `is_terminal`, `IDLE_SLEEP`, `BATCH`; add `use longbox_db::issue_repo;` if not present)

```rust
/// Spawn the Metron issue-linking resolver (separate from the series-linker).
pub fn spawn_metron_issue_linker(db: Pool, metron: Arc<MetronClient>) {
    tokio::spawn(issue_link_loop(db, metron));
}

async fn issue_link_loop(db: Pool, metron: Arc<MetronClient>) {
    loop {
        let batch = match series_repo::list_metron_issue_link_candidates(&db, BATCH).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "longbox_metron_link", error = %e, "issue-link candidate query failed");
                tokio::time::sleep(IDLE_SLEEP).await;
                continue;
            }
        };
        if batch.is_empty() {
            tokio::time::sleep(IDLE_SLEEP).await;
            continue;
        }
        let (mut series_marked, mut issues_linked) = (0usize, 0usize);
        for (series_id, metron_id) in &batch {
            // A non-numeric metron_id is bad data — mark done so it can't churn.
            let Ok(mid) = metron_id.parse::<i64>() else {
                let _ = series_repo::mark_metron_issues_linked(&db, *series_id).await;
                series_marked += 1;
                continue;
            };
            match metron.fetch_issues_by_series_id(mid).await {
                Ok(refs) => {
                    match issue_repo::list_by_series(&db, *series_id).await {
                        Ok(ours) => {
                            let rows: Vec<(i64, String, bool)> = ours
                                .iter()
                                .map(|i| (i.id, i.number.clone(), i.metron_issue_id.is_some()))
                                .collect();
                            for (issue_id, m_issue_id) in match_issue_links(&rows, &refs) {
                                if series_repo_set_issue_ok(&db, issue_id, m_issue_id).await {
                                    issues_linked += 1;
                                }
                            }
                            let _ = series_repo::mark_metron_issues_linked(&db, *series_id).await;
                            series_marked += 1;
                        }
                        Err(e) => tracing::warn!(target: "longbox_metron_link",
                            series_id, error = %e, "list_by_series failed; will retry"),
                    }
                }
                // Permanent per-series fetch error — mark done (no issues), no churn.
                Err(e) if is_terminal(&e) => {
                    let _ = series_repo::mark_metron_issues_linked(&db, *series_id).await;
                    series_marked += 1;
                    tracing::debug!(target: "longbox_metron_link",
                        series_id, error = %e, "terminal metron error; marked issues-linked (no data)");
                }
                // Transient — leave unmarked, retry next pass.
                Err(e) => tracing::warn!(target: "longbox_metron_link",
                    series_id, error = %e, "issue-list fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_metron_link", series_marked, issues_linked, "metron issue-link pass");
        // No progress this pass (all transient errors) — back off.
        if series_marked == 0 {
            tokio::time::sleep(IDLE_SLEEP).await;
        }
    }
}

/// Small helper so the loop body stays readable.
async fn series_repo_set_issue_ok(db: &Pool, issue_id: i64, metron_issue_id: i64) -> bool {
    issue_repo::set_metron_issue_id(db, issue_id, &metron_issue_id.to_string())
        .await
        .is_ok()
}
```

> `is_terminal`, `IDLE_SLEEP`, `BATCH`, and the `use` of `series_repo`/`Pool`/`MetronClient`/`Arc` already exist in this file from the series-linker — reuse them. Add only the new imports (`issue_repo`, `IssueNumber`, `MetronIssueRef`).

- [ ] **Step 6: Spawn at bootstrap** — in `longbox-web/src/bootstrap.rs`, right after the existing `spawn_metron_linker` gated spawn, add (same gate/vars):

```rust
    if let Some(ref m) = metron {
        crate::metron_link::spawn_metron_issue_linker(db.clone(), Arc::clone(m));
    }
```

(If the series-linker spawn is already inside an `if let Some(ref m) = metron { ... }` block, add the second spawn line inside that same block instead of a second `if let`.)

- [ ] **Step 7: Build + test + clippy**

```bash
SQLX_OFFLINE=true cargo build --workspace
SQLX_OFFLINE=true cargo test --workspace
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean, green. The resolver loop is HTTP-bound; its correctness rests on `match_issue_links` (unit-tested here) + the DB tests (Task 3).

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add longbox-web/src/metron_link.rs longbox-web/src/bootstrap.rs
git commit -m "feat(web): Metron issue-linking resolver + spawn"
```

- [ ] **Step 9: REVIEWER GATE** — run `superpowers:code-reviewer` over Commits 1–4. Focus: `match_issue_links` correctness (number match, dup→first, already-linked/unmatched skipped); resolver convergence (Ok→link+mark; terminal Err→mark, no churn; transient Err→retry; bad metron_id→mark; no-progress backoff); one bulk call per series (paginated); the `set_metron_issue_id` race guard; spawn gated on `metron.is_some()`; and that the finished series-linker is untouched. Address findings.

---

## Final verification

- [ ] `SQLX_OFFLINE=true cargo test --workspace` — green.
- [ ] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check` — clean.
- [ ] **Live smoke (post-deploy):** rebuild + `up -d`, watch `tracing` target `longbox_metron_link` log `"metron issue-link pass"` with nonzero `issues_linked`; confirm `SELECT COUNT(*) FROM issues WHERE metron_issue_id IS NOT NULL` climbs from 0, and `SELECT COUNT(*) FROM series WHERE metron_issues_linked_at IS NOT NULL` climbs toward the ~606 Metron-linked series.

## Out of scope
- Metron credits (next feature — needs its own kickoff for the creator name-dedup).
- MetronInfo.xml export.
- Re-linking issues once linked (no staleness re-scan in v1).

## Self-review notes
- **Decision coverage:** standalone (no credits code) ✓; separate resolver + `metron_issues_linked_at` marker, series-linker untouched ✓; all issues in linked series (no owned filter) ✓; deterministic `IssueNumber::matches`, unmatched NULL, dup→first (Task 4 test) ✓; paginated fetch (cloned loop) ✓.
- **Type consistency:** `MetronIssueRef { metron_issue_id: i64, issue_number: String }` (Task 2) → `fetch_issues_by_series_id` return → `match_issue_links(_, &[MetronIssueRef])` (Task 4). `list_metron_issue_link_candidates -> Vec<(i64, String)>` (series_id, metron_id) → resolver parses `metron_id` to `i64` for the fetch. `set_metron_issue_id(_, issue_id, &str)` — resolver passes `&metron_issue_id.to_string()`.
- **No-churn:** every series gets `mark_metron_issues_linked` on the Ok path AND the terminal-error/bad-id paths; only transient errors leave it unmarked to retry — same shape as the series-linker.
