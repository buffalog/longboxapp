# Creator Credits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Role-attributed creator ingestion from ComicVine per-issue credits + in-library creator search (search creators, see their owned issues grouped by series, filter by role).

**Architecture:** A new `creators` + `issue_credits` schema, a CV per-issue-detail fetch that pulls `person_credits` (splitting CV's comma-delimited role strings into atomic rows at ingestion), a continuous low-priority background resolver that drains `credits_fetched=0` owned issues behind the existing `BackgroundCvClient` throttle, a read API over the credit graph (owned-only), and a SvelteKit `/creators` browse UI.

**Tech Stack:** Rust, sqlx (SQLite, compile-checked, offline `.sqlx`), Axum 0.7, tokio, `governor` (existing rate limiter), SvelteKit.

**Locked kickoff decisions:**
1. CV roles are comma-delimited per person ("artist, colorist, cover") — **split into atomic rows at ingestion** (trim + lowercase). One `issue_credits` row per person + atomic role + issue.
2. `commit_merge` does **NOT** fetch credits inline — new issues land with `credits_fetched=0` (the column DEFAULT) and the background resolver picks them up. (Old Commit 4 inline hook is dropped.)
3. Backfill is a **continuous low-priority resolver** (no 2am gate) behind `BackgroundCvClient` (~120 req/h → ~51h cumulative for ~6,180 owned issues; new subscribes resolve within minutes).
4. Backfill scope = `credits_fetched=0 AND cv_issue_id IS NOT NULL AND <owned>`. Real column is **`cv_issue_id`** (not `cv_id`). The ~36 no-CV-id issues are excluded.
5. `upsert_creator` = `INSERT ... ON CONFLICT(cv_person_id) DO UPDATE SET name=excluded.name`.
6. Top-level nav item "Creators".
7. No re-fetch path (accept staleness).
8. Raw atomic roles in the facet (no canonical grouping).

**Pre-flight:**
```bash
git checkout -b feat/creator-credits
SQLX_OFFLINE=true cargo test --workspace   # baseline green
# sqlx prepare DB (reused in Commits 3 & 5):
export DATABASE_URL="sqlite:/tmp/lb-credits-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
```
Use `SQLX_OFFLINE=true` for all `cargo build`/`cargo test`. After any task that adds/changes a `query!`/`query_as!`, regenerate metadata: `cargo sqlx prepare --workspace` (with `DATABASE_URL` set + the migration applied) and commit the `.sqlx/` changes.

**Reviewer gates** (superpowers:code-reviewer, NOT longbox-reviewer): after **Commit 4** (resolver — touches the shared CV budget + background concurrency) and after **Commit 5** (API surface + query correctness).

---

## File Structure

| File | Responsibility | Commit |
|------|----------------|--------|
| `longbox-db/migrations/20260701000000_add_creator_credits.sql` (create) | `creators` + `issue_credits` tables, `issues.credits_fetched` | 1 |
| `longbox-comicvine/src/models.rs` (modify) | raw `CvIssueCreditsRaw` + `CvCreditRaw` DTOs | 2 |
| `longbox-comicvine/src/projection.rs` (modify) | public `CvPersonCredit` + `project_issue_credits` (atomic-role split) | 2 |
| `longbox-comicvine/src/client.rs` (modify) | `ComicVineClient::fetch_issue_credits` | 2 |
| `longbox-cv-enrichment/src/background.rs` (modify) | `BackgroundCvClient::fetch_issue_credits` throttled delegate | 2 |
| `longbox-db/src/creator_repo.rs` (create) | creators upsert, `insert_issue_credits`, work-list, search/detail/issues queries | 3,4,5 |
| `longbox-db/src/lib.rs` (modify) | export `creator_repo` + types | 3 |
| `longbox-db/tests/creators.rs` (create) | repo integration tests | 3,4,5 |
| `longbox-cv-enrichment/src/credits_resolver.rs` (create) | continuous resolver loop + spawn | 4 |
| `longbox-cv-enrichment/src/lib.rs` (modify) | `pub mod credits_resolver;` | 4 |
| `longbox-web/src/bootstrap.rs` (modify) | spawn the resolver | 4 |
| `longbox-web/src/routes/creators.rs` (create) | 3 GET handlers + `router()` | 5 |
| `longbox-web/src/routes/mod.rs` (modify) | register `creators::router()` | 5 |
| `longbox-frontend/src/lib/api/creators.ts` (create) | typed API client | 6 |
| `longbox-frontend/src/routes/creators/+page.{svelte,ts}` (create) | search page | 6 |
| `longbox-frontend/src/routes/creators/[id]/+page.{svelte,ts}` (create) | detail page | 6 |
| `longbox-frontend/src/lib/components/NavBar.svelte` (modify) | "Creators" nav item | 6 |

**Shared SQL predicate — "owned issue"** (used in the resolver work-list and every read query):
```sql
EXISTS (SELECT 1 FROM files f WHERE f.issue_id = i.id AND f.status = 'owned' AND f.is_present = 1)
```

---

## Commit 1 — Schema

### Task 1: Migration

**Files:** Create `longbox-db/migrations/20260701000000_add_creator_credits.sql`

- [ ] **Step 1: Write the migration** (all three DDL changes in one atomic file)

```sql
-- Creator credits (role-attributed). `creators` dedupes a person across
-- sources via cv_person_id; `issue_credits` is the many-to-many person+role+
-- issue graph (one row per ATOMIC role — CV's comma-delimited role strings
-- are split at ingestion). `issues.credits_fetched` gates the background
-- credits resolver so each issue's per-issue CV detail is fetched once.
CREATE TABLE creators (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL,
    cv_person_id     INTEGER UNIQUE,
    metron_person_id INTEGER UNIQUE,
    created_at       TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_creators_name ON creators(name COLLATE NOCASE);

CREATE TABLE issue_credits (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    creator_id  INTEGER NOT NULL REFERENCES creators(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    UNIQUE(issue_id, creator_id, role)
);
CREATE INDEX idx_issue_credits_creator ON issue_credits(creator_id);
CREATE INDEX idx_issue_credits_issue   ON issue_credits(issue_id);

ALTER TABLE issues ADD COLUMN credits_fetched BOOLEAN NOT NULL DEFAULT 0;
```

- [ ] **Step 2: Verify it applies + existing build unaffected**

```bash
export DATABASE_URL="sqlite:/tmp/lb-credits-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
SQLX_OFFLINE=true cargo test -p longbox-db migration
```
Expected: migration applies; `migration_creates_all_tables`/`_indexes` pass. (No `cargo sqlx prepare` needed yet — no `query!` references the new tables, and `IssueRow` selects explicit columns so the new `credits_fetched` column doesn't affect existing queries.)

- [ ] **Step 3: Commit**

```bash
git add longbox-db/migrations/20260701000000_add_creator_credits.sql
git commit -m "feat(db): creators + issue_credits schema, issues.credits_fetched"
```

---

## Commit 2 — CV per-issue credits DTO + fetch

### Task 2: Raw DTO + atomic-role-splitting projection

**Files:** Modify `longbox-comicvine/src/models.rs`, `longbox-comicvine/src/projection.rs`

- [ ] **Step 1: Write the failing projection test**

Add to `longbox-comicvine/src/projection.rs` (a new `#[cfg(test)] mod credit_tests`):

```rust
#[cfg(test)]
mod credit_tests {
    use super::*;
    use crate::models::{CvCreditRaw, CvIssueCreditsRaw};

    #[test]
    fn splits_comma_delimited_roles_into_atomic_lowercase_rows() {
        // CV packs multi-role people into one comma-delimited `role` string.
        let raw = CvIssueCreditsRaw {
            id: 42,
            person_credits: vec![
                CvCreditRaw { id: 97470, name: "Bob Quinn".into(), role: "artist, colorist, cover".into() },
                CvCreditRaw { id: 130355, name: "Ethan S. Parker".into(), role: "Writer".into() },
                CvCreditRaw { id: 1, name: "Blank".into(), role: "  ".into() }, // whitespace-only -> dropped
            ],
        };
        let out = project_issue_credits(raw);
        assert_eq!(out, vec![
            CvPersonCredit { cv_person_id: 97470, name: "Bob Quinn".into(), role: "artist".into() },
            CvPersonCredit { cv_person_id: 97470, name: "Bob Quinn".into(), role: "colorist".into() },
            CvPersonCredit { cv_person_id: 97470, name: "Bob Quinn".into(), role: "cover".into() },
            CvPersonCredit { cv_person_id: 130355, name: "Ethan S. Parker".into(), role: "writer".into() },
        ]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-comicvine splits_comma_delimited`
Expected: FAIL to compile (`CvIssueCreditsRaw`/`CvCreditRaw`/`CvPersonCredit`/`project_issue_credits` undefined).

- [ ] **Step 3: Add the raw DTOs** in `longbox-comicvine/src/models.rs` (after `CvIssueFull`, around models.rs:107)

```rust
/// One entry of an issue's `person_credits` array, from
/// `/issue/4000-<id>/?field_list=id,person_credits`. CV packs multiple
/// roles for one person into a single comma-delimited `role` string.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvCreditRaw {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
}

/// The `results` object of a per-issue credits fetch. Only the fields
/// requested via `field_list`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvIssueCreditsRaw {
    pub id: i64,
    #[serde(default)]
    pub person_credits: Vec<CvCreditRaw>,
}
```

- [ ] **Step 4: Add the public type + projection** in `longbox-comicvine/src/projection.rs` (after `CvIssueDetail`, around projection.rs:44)

```rust
/// One atomic person+role credit on an issue (CV's comma-delimited role
/// strings are exploded into one of these per role). `cv_person_id` is the
/// CV person resource id used to dedupe creators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvPersonCredit {
    pub cv_person_id: i64,
    pub name: String,
    pub role: String,
}
```

Add the import to the existing `use crate::models::{...}` line in projection.rs (add `CvIssueCreditsRaw`), then add the projection fn:

```rust
/// Explode a raw per-issue credits payload into atomic `CvPersonCredit`
/// rows: split each comma-delimited `role` string, trim + lowercase each
/// role, drop empties. One output entry per person+atomic-role; a person's
/// order and CV's order are preserved.
pub(crate) fn project_issue_credits(raw: CvIssueCreditsRaw) -> Vec<CvPersonCredit> {
    let mut out = Vec::new();
    for c in raw.person_credits {
        for role in c.role.split(',') {
            let role = role.trim().to_lowercase();
            if role.is_empty() {
                continue;
            }
            out.push(CvPersonCredit { cv_person_id: c.id, name: c.name.clone(), role });
        }
    }
    out
}
```

- [ ] **Step 5: Run to verify pass**

Run: `SQLX_OFFLINE=true cargo test -p longbox-comicvine splits_comma_delimited`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add longbox-comicvine/src/models.rs longbox-comicvine/src/projection.rs
git commit -m "feat(comicvine): per-issue credits DTO + atomic-role projection"
```

### Task 3: `fetch_issue_credits` on the CV client + background delegate

**Files:** Modify `longbox-comicvine/src/client.rs`, `longbox-cv-enrichment/src/background.rs`

- [ ] **Step 1: Add the client method** in `longbox-comicvine/src/client.rs` (inside `impl ComicVineClient`, after `fetch_issues`, around client.rs:251). Mirror `fetch_volume`'s shape exactly; the issue resource prefix is `4000-`, and `field_list` is passed as a query param via `build_url`.

```rust
/// Fetch the per-issue `person_credits` for one issue (the bulk
/// `issues/` list does not carry credits). Returns atomic person+role
/// credits. `field_list=id,person_credits` keeps the payload minimal.
#[instrument(target = "longbox_comicvine", skip(self))]
pub async fn fetch_issue_credits(&self, cv_issue_id: i64) -> Result<Vec<CvPersonCredit>, CvError> {
    let path = format!("issue/4000-{cv_issue_id}/");
    let url = self.build_url(&path, &[("field_list", "id,person_credits")])?;
    let body = self.execute_with_retry(url).await?;
    let envelope = parse_envelope::<CvIssueCreditsRaw>(&body)?;
    let raw = unwrap_envelope_results(envelope, &body)?;
    Ok(project_issue_credits(raw))
}
```

Ensure the `use` for the projection items is in scope — `CvPersonCredit` and `project_issue_credits` come from `crate::projection`, and `CvIssueCreditsRaw` from `crate::models`. Add them to the existing imports at the top of client.rs if not already wildcarded (match how `CvVolumeFull`/`project_volume` are imported).

- [ ] **Step 2: Add the background delegate** in `longbox-cv-enrichment/src/background.rs` (inside `impl BackgroundCvClient`, after `fetch_issues`, around background.rs:84). Import `CvPersonCredit` from `longbox_comicvine` at the top.

```rust
/// Background `fetch_issue_credits`. Acquires the background gate, then
/// delegates — used by the credits resolver so per-issue credit fetches
/// run at low priority under the shared 180/h limiter.
pub async fn fetch_issue_credits(&self, cv_issue_id: i64) -> Result<Vec<CvPersonCredit>, CvError> {
    self.background_gate.until_ready().await;
    self.inner.fetch_issue_credits(cv_issue_id).await
}
```

- [ ] **Step 3: Build (HTTP method — no unit test; exercised by the resolver in Commit 4)**

Run: `SQLX_OFFLINE=true cargo build -p longbox-comicvine -p longbox-cv-enrichment`
Expected: clean compile. Confirm `CvPersonCredit` is exported from `longbox-comicvine`'s public API (it's `pub` on a `pub` projection type — verify `longbox_comicvine::CvPersonCredit` resolves; if the crate re-exports projection types via `lib.rs`, add `CvPersonCredit` to that re-export list alongside `CvIssueDetail`).

- [ ] **Step 4: Commit**

```bash
git add longbox-comicvine/src/client.rs longbox-comicvine/src/lib.rs longbox-cv-enrichment/src/background.rs
git commit -m "feat(comicvine): fetch_issue_credits + background delegate"
```

---

## Commit 3 — Persist layer (`creator_repo`)

### Task 4: `upsert_creator` + `insert_issue_credits`

**Files:** Create `longbox-db/src/creator_repo.rs`; modify `longbox-db/src/lib.rs`; create `longbox-db/tests/creators.rs`

- [ ] **Step 1: Write the failing integration test** — create `longbox-db/tests/creators.rs`

```rust
mod common;
use common::fresh_pool;
use longbox_comicvine::CvPersonCredit;
use longbox_db::{creator_repo, file_repo, issue_repo, library_root_repo, series_repo,
    NewFile, NewIssue, NewLibraryRoot, NewSeries};

async fn seed_owned_issue(pool: &sqlx::SqlitePool, cv_issue_id: i64) -> i64 {
    let root = library_root_repo::insert(pool, NewLibraryRoot { path: format!("/c{cv_issue_id}") })
        .await.unwrap();
    let sid = series_repo::insert(pool, NewSeries {
        cv_id: Some(cv_issue_id * 10), metron_id: None, title: "Deadly Class".into(),
        sort_title: "deadly class".into(), start_year: Some(2014),
        publisher: Some("Image".into()), description: None, cover_url: None,
    }).await.unwrap().id;
    let iid = issue_repo::insert(pool, NewIssue {
        series_id: sid, cv_issue_id: Some(cv_issue_id), metron_issue_id: None,
        number: "1".into(), title: None, cover_date: Some("2014-01-01".into()),
        summary: None, cover_url: None,
    }).await.unwrap().id;
    file_repo::insert(pool, NewFile {
        issue_id: Some(iid), library_root_id: root.id, relative_path: format!("d{cv_issue_id}.cbz"),
        status: "owned".into(), ..NewFile::minimal()
    }).await.unwrap();
    iid
}

#[tokio::test]
async fn insert_issue_credits_dedupes_creator_and_sets_fetched() {
    let pool = fresh_pool().await;
    let iid = seed_owned_issue(&pool, 1001).await;
    let credits = vec![
        CvPersonCredit { cv_person_id: 97470, name: "Bob Quinn".into(), role: "artist".into() },
        CvPersonCredit { cv_person_id: 97470, name: "Bob Quinn".into(), role: "cover".into() },
        CvPersonCredit { cv_person_id: 55, name: "Rick Remender".into(), role: "writer".into() },
    ];
    creator_repo::insert_issue_credits(&pool, iid, &credits).await.unwrap();

    // One creator per cv_person_id (Bob Quinn appears once despite 2 roles).
    let n_creators: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM creators").fetch_one(&pool).await.unwrap();
    assert_eq!(n_creators, 2);
    // Three atomic credit rows.
    let n_credits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_credits").fetch_one(&pool).await.unwrap();
    assert_eq!(n_credits, 3);
    // Idempotent re-insert: no new rows, no error.
    creator_repo::insert_issue_credits(&pool, iid, &credits).await.unwrap();
    let n2: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_credits").fetch_one(&pool).await.unwrap();
    assert_eq!(n2, 3);
    // credits_fetched flipped.
    let fetched: bool = sqlx::query_scalar("SELECT credits_fetched FROM issues WHERE id=?")
        .bind(iid).fetch_one(&pool).await.unwrap();
    assert!(fetched);
}

#[tokio::test]
async fn insert_empty_credits_marks_fetched_with_no_rows() {
    let pool = fresh_pool().await;
    let iid = seed_owned_issue(&pool, 1002).await;
    creator_repo::insert_issue_credits(&pool, iid, &[]).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_credits").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
    let fetched: bool = sqlx::query_scalar("SELECT credits_fetched FROM issues WHERE id=?")
        .bind(iid).fetch_one(&pool).await.unwrap();
    assert!(fetched, "empty credits (CV NotFound case) must still mark the issue done");
}
```

> If `NewFile::minimal()` / `NewFile` field names differ in this repo, mirror how `longbox-db/tests/files.rs` constructs a `NewFile` (read that file's `seed` helper) — the only requirements are `issue_id = Some(iid)` and `status = "owned"` with `is_present = 1` (the owned predicate). Adjust the helper to the real `NewFile` shape; do not change the assertions.

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db --test creators`
Expected: FAIL to compile (`creator_repo` undefined).

- [ ] **Step 3: Create `longbox-db/src/creator_repo.rs`**

```rust
//! Creator + per-issue credit persistence and read queries. Credits are
//! role-atomic (CV's comma-delimited role strings are split upstream).
use sqlx::SqliteExecutor;

use longbox_comicvine::CvPersonCredit;

use crate::error::Result;
use crate::pool::Pool;

/// Insert-or-update a creator by CV person id, returning its local id.
/// ON CONFLICT keeps the name fresh (CV name corrections propagate).
pub async fn upsert_creator<'e, E>(executor: E, cv_person_id: i64, name: &str) -> Result<i64>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"INSERT INTO creators (cv_person_id, name) VALUES (?, ?)
           ON CONFLICT(cv_person_id) DO UPDATE SET name = excluded.name
           RETURNING id AS "id!: i64""#,
        cv_person_id,
        name,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.id)
}

/// Persist a fully-resolved set of atomic credits for one issue and flip
/// `credits_fetched`. Transactional + idempotent: creators dedupe on
/// cv_person_id, credit rows `INSERT OR IGNORE` against the UNIQUE
/// (issue_id, creator_id, role). An empty slice marks the issue done with
/// no rows (the CV-NotFound path).
pub async fn insert_issue_credits(pool: &Pool, issue_id: i64, credits: &[CvPersonCredit]) -> Result<()> {
    let mut tx = pool.begin().await?;
    for c in credits {
        let creator_id = upsert_creator(&mut *tx, c.cv_person_id, &c.name).await?;
        sqlx::query!(
            r#"INSERT OR IGNORE INTO issue_credits (issue_id, creator_id, role) VALUES (?, ?, ?)"#,
            issue_id,
            creator_id,
            c.role,
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(r#"UPDATE issues SET credits_fetched = 1 WHERE id = ?"#, issue_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
```

- [ ] **Step 4: Export** in `longbox-db/src/lib.rs` — add `pub mod creator_repo;` (in the `pub mod` block ~line 8-28). No type re-export needed yet.

- [ ] **Step 5: Regenerate sqlx metadata + run tests**

```bash
export DATABASE_URL="sqlite:/tmp/lb-credits-prepare.db?mode=rwc"
cargo sqlx prepare --workspace
SQLX_OFFLINE=true cargo test -p longbox-db --test creators
```
Expected: both tests PASS. Review `git diff --stat .sqlx/` — should add ~3 query json files (the upsert RETURNING, the INSERT OR IGNORE, the UPDATE), nothing unrelated.

- [ ] **Step 6: Commit**

```bash
git add longbox-db/src/creator_repo.rs longbox-db/src/lib.rs longbox-db/tests/creators.rs .sqlx
git commit -m "feat(db): creator_repo upsert + insert_issue_credits"
```

---

## Commit 4 — Continuous credits resolver  ⟶ REVIEWER GATE after this commit

### Task 5: Work-list query

**Files:** Modify `longbox-db/src/creator_repo.rs`, `longbox-db/tests/creators.rs`

- [ ] **Step 1: Add the failing test** to `longbox-db/tests/creators.rs`

```rust
#[tokio::test]
async fn list_issues_needing_credits_filters_owned_unfetched_with_cv_id() {
    let pool = fresh_pool().await;
    let owned = seed_owned_issue(&pool, 2001).await;            // owned, cv_id, not fetched -> included
    let already = seed_owned_issue(&pool, 2002).await;
    creator_repo::insert_issue_credits(&pool, already, &[]).await.unwrap(); // fetched -> excluded
    // owned but NO cv_issue_id -> excluded
    let sid = series_repo::insert(&pool, NewSeries {
        cv_id: None, metron_id: None, title: "X".into(), sort_title: "x".into(),
        start_year: None, publisher: None, description: None, cover_url: None,
    }).await.unwrap().id;
    let no_cv = issue_repo::insert(&pool, NewIssue {
        series_id: sid, cv_issue_id: None, metron_issue_id: None, number: "1".into(),
        title: None, cover_date: None, summary: None, cover_url: None,
    }).await.unwrap().id;
    let root = library_root_repo::insert(&pool, NewLibraryRoot { path: "/nocv".into() }).await.unwrap();
    file_repo::insert(&pool, NewFile { issue_id: Some(no_cv), library_root_id: root.id,
        relative_path: "n.cbz".into(), status: "owned".into(), ..NewFile::minimal() }).await.unwrap();

    let work = creator_repo::list_issues_needing_credits(&pool, 50).await.unwrap();
    let ids: Vec<i64> = work.iter().map(|w| w.issue_id).collect();
    assert_eq!(ids, vec![owned], "only the owned, cv-keyed, unfetched issue");
    assert_eq!(work[0].cv_issue_id, 2001);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db list_issues_needing_credits`
Expected: FAIL (undefined `list_issues_needing_credits` / `IssueNeedingCredits`).

- [ ] **Step 3: Implement** in `longbox-db/src/creator_repo.rs`

```rust
/// One issue awaiting credit resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueNeedingCredits {
    pub issue_id: i64,
    pub cv_issue_id: i64,
}

/// Owned, CV-keyed issues whose credits haven't been fetched yet, oldest
/// first. Drives the background resolver. Skips non-owned (out of scope for
/// search) and the ~36 issues with no cv_issue_id (can't be fetched).
pub async fn list_issues_needing_credits<'e, E>(
    executor: E,
    limit: i64,
) -> Result<Vec<IssueNeedingCredits>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT i.id AS "issue_id!: i64", i.cv_issue_id AS "cv_issue_id!: i64"
           FROM issues i
           WHERE i.credits_fetched = 0
             AND i.cv_issue_id IS NOT NULL
             AND EXISTS (SELECT 1 FROM files f
                         WHERE f.issue_id = i.id AND f.status = 'owned' AND f.is_present = 1)
           ORDER BY i.id ASC
           LIMIT ?"#,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter()
        .map(|r| IssueNeedingCredits { issue_id: r.issue_id, cv_issue_id: r.cv_issue_id })
        .collect())
}
```

- [ ] **Step 4: sqlx prepare + run**

```bash
export DATABASE_URL="sqlite:/tmp/lb-credits-prepare.db?mode=rwc"
cargo sqlx prepare --workspace
SQLX_OFFLINE=true cargo test -p longbox-db --test creators
```
Expected: all creators tests PASS.

- [ ] **Step 5: Commit**

```bash
git add longbox-db/src/creator_repo.rs longbox-db/tests/creators.rs .sqlx
git commit -m "feat(db): list_issues_needing_credits work-list query"
```

### Task 6: Resolver task + spawn wiring

**Files:** Create `longbox-cv-enrichment/src/credits_resolver.rs`; modify `longbox-cv-enrichment/src/lib.rs`, `longbox-web/src/bootstrap.rs`

- [ ] **Step 1: Create `longbox-cv-enrichment/src/credits_resolver.rs`**

```rust
//! Continuous low-priority credits resolver. Drains owned, CV-keyed issues
//! whose `credits_fetched = 0` by fetching their per-issue `person_credits`
//! behind the shared `BackgroundCvClient` throttle (~120 req/h, leaving the
//! interactive 180/h budget headroom). Fire-and-forget; no settings gate.
use std::sync::Arc;
use std::time::Duration;

use longbox_comicvine::{ComicVineClient, CvError};
use longbox_db::{creator_repo, Pool};

use crate::background::BackgroundCvClient;

/// Idle re-check interval when there's no work (new subscribes show up here).
const IDLE_SLEEP: Duration = Duration::from_secs(300);
/// Background call spacing (matches the enrichment worker default).
const REQUEST_INTERVAL: Duration = Duration::from_secs(30);
/// Issues per work-list batch.
const BATCH: i64 = 50;

/// Spawn the resolver onto the tokio runtime. `inner_cv` is the shared
/// 180/h CV client (same Arc the enrichment worker uses).
pub fn spawn_credits_resolver(db: Pool, inner_cv: Arc<ComicVineClient>) {
    tokio::spawn(credits_loop(db, inner_cv));
}

async fn credits_loop(db: Pool, inner_cv: Arc<ComicVineClient>) {
    let bg = BackgroundCvClient::new(inner_cv, REQUEST_INTERVAL);
    loop {
        let batch = match creator_repo::list_issues_needing_credits(&db, BATCH).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "longbox_credits", error = %e, "work-list query failed");
                tokio::time::sleep(IDLE_SLEEP).await;
                continue;
            }
        };
        if batch.is_empty() {
            tokio::time::sleep(IDLE_SLEEP).await;
            continue;
        }
        let mut processed = 0usize;
        for item in &batch {
            match bg.fetch_issue_credits(item.cv_issue_id).await {
                Ok(credits) => {
                    match creator_repo::insert_issue_credits(&db, item.issue_id, &credits).await {
                        Ok(()) => processed += 1,
                        Err(e) => tracing::warn!(target: "longbox_credits",
                            issue_id = item.issue_id, error = %e, "persist failed"),
                    }
                }
                // CV doesn't have the issue — mark done (zero credits) so it
                // doesn't churn the work-list forever.
                Err(CvError::NotFound) => {
                    let _ = creator_repo::insert_issue_credits(&db, item.issue_id, &[]).await;
                }
                // Transient (rate-limit / network / 5xx) — leave for retry.
                Err(e) => tracing::warn!(target: "longbox_credits",
                    cv_issue_id = item.cv_issue_id, error = %e, "credit fetch failed; will retry"),
            }
        }
        tracing::info!(target: "longbox_credits", processed, "credits resolver pass");
    }
}
```

- [ ] **Step 2: Export** in `longbox-cv-enrichment/src/lib.rs` — add `pub mod credits_resolver;` (alongside `pub mod background;` ~line 37).

- [ ] **Step 3: Spawn at bootstrap** — in `longbox-web/src/bootstrap.rs`, immediately after the existing `longbox_cv_enrichment::spawn(...)` call (~bootstrap.rs:176-180), add:

```rust
    longbox_cv_enrichment::credits_resolver::spawn_credits_resolver(db.clone(), Arc::clone(&cv_arc));
```

(`db`, `cv_arc`, and `Arc` are all already in scope at that point.)

- [ ] **Step 4: Build the workspace**

Run: `SQLX_OFFLINE=true cargo build --workspace`
Expected: clean compile. The resolver's behavior (HTTP-bound) is covered by the work-list test (Task 5) + the persist tests (Task 4); the loop glue is verified at the reviewer gate.

- [ ] **Step 5: Commit**

```bash
git add longbox-cv-enrichment/src/credits_resolver.rs longbox-cv-enrichment/src/lib.rs longbox-web/src/bootstrap.rs
git commit -m "feat(cv-enrichment): continuous credits resolver + spawn"
```

- [ ] **Step 6: REVIEWER GATE** — run `superpowers:code-reviewer` over Commits 1–4 (schema → DTO → persist → resolver). Focus: resolver can't starve the interactive CV budget (it uses the background gate), idempotency/no-churn (NotFound marks done; transient retries), tx correctness in `insert_issue_credits`. Address findings before Commit 5.

---

## Commit 5 — Read API  ⟶ REVIEWER GATE after this commit

### Task 7: Read queries (`creator_repo`)

**Files:** Modify `longbox-db/src/creator_repo.rs`, `longbox-db/tests/creators.rs`

- [ ] **Step 1: Add failing tests** to `longbox-db/tests/creators.rs`

```rust
#[tokio::test]
async fn search_and_detail_count_owned_only() {
    let pool = fresh_pool().await;
    let i1 = seed_owned_issue(&pool, 3001).await; // series cv 30010
    let i2 = seed_owned_issue(&pool, 3002).await; // series cv 30020
    let remender = vec![CvPersonCredit { cv_person_id: 55, name: "Rick Remender".into(), role: "writer".into() }];
    creator_repo::insert_issue_credits(&pool, i1, &remender).await.unwrap();
    creator_repo::insert_issue_credits(&pool, i2, &remender).await.unwrap();

    let hits = creator_repo::search_creators(&pool, "remender").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Rick Remender");
    assert_eq!(hits[0].issue_count, 2);
    assert_eq!(hits[0].series_count, 2);

    let detail = creator_repo::creator_detail(&pool, hits[0].id).await.unwrap().unwrap();
    assert_eq!(detail.roles, vec![creator_repo::RoleCount { role: "writer".into(), count: 2 }]);
    assert_eq!(detail.series.len(), 2);

    let issues = creator_repo::creator_issues(&pool, hits[0].id, None, None, 1).await.unwrap();
    assert_eq!(issues.len(), 2);
    // role filter
    let none = creator_repo::creator_issues(&pool, hits[0].id, Some("artist"), None, 1).await.unwrap();
    assert!(none.is_empty());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db search_and_detail_count_owned_only`
Expected: FAIL (undefined functions/types).

- [ ] **Step 3: Implement the read structs + queries** in `longbox-db/src/creator_repo.rs`

```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorSearchRow {
    pub id: i64,
    pub name: String,
    pub cv_person_id: Option<i64>,
    pub series_count: i64,
    pub issue_count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RoleCount { pub role: String, pub count: i64 }

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorSeries {
    pub series_id: i64,
    pub name: String,
    pub issue_count: i64,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorDetail {
    pub id: i64,
    pub name: String,
    pub cv_person_id: Option<i64>,
    pub roles: Vec<RoleCount>,
    pub series: Vec<CreatorSeries>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreatorIssueRow {
    pub issue_id: i64,
    pub series_name: String,
    pub issue_number: String,
    pub cover_date: Option<String>,
    pub cover_url: Option<String>,
    pub role: String,
}

/// Creator name search (owned issues only). `q` is wrapped in `%…%`.
pub async fn search_creators<'e, E>(executor: E, q: &str) -> Result<Vec<CreatorSearchRow>>
where E: SqliteExecutor<'e> {
    let like = format!("%{q}%");
    let rows = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", c.name AS "name!", c.cv_person_id,
                  COUNT(DISTINCT i.series_id) AS "series_count!: i64",
                  COUNT(DISTINCT i.id)        AS "issue_count!: i64"
           FROM creators c
           JOIN issue_credits ic ON ic.creator_id = c.id
           JOIN issues i         ON i.id = ic.issue_id
           WHERE c.name LIKE ? COLLATE NOCASE
             AND EXISTS (SELECT 1 FROM files f
                         WHERE f.issue_id = i.id AND f.status='owned' AND f.is_present=1)
           GROUP BY c.id
           ORDER BY issue_count DESC
           LIMIT 20"#,
        like,
    ).fetch_all(executor).await?;
    Ok(rows.into_iter().map(|r| CreatorSearchRow {
        id: r.id, name: r.name, cv_person_id: r.cv_person_id,
        series_count: r.series_count, issue_count: r.issue_count,
    }).collect())
}

/// Creator detail: name + role facet + series list (owned issues only).
pub async fn creator_detail(pool: &Pool, id: i64) -> Result<Option<CreatorDetail>> {
    let base = sqlx::query!(
        r#"SELECT id AS "id!: i64", name AS "name!", cv_person_id FROM creators WHERE id = ?"#, id
    ).fetch_optional(&*pool).await?;
    let Some(base) = base else { return Ok(None) };

    let roles = sqlx::query!(
        r#"SELECT ic.role AS "role!", COUNT(DISTINCT ic.issue_id) AS "count!: i64"
           FROM issue_credits ic JOIN issues i ON i.id = ic.issue_id
           WHERE ic.creator_id = ?
             AND EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id AND f.status='owned' AND f.is_present=1)
           GROUP BY ic.role ORDER BY count DESC"#, id
    ).fetch_all(&*pool).await?
     .into_iter().map(|r| RoleCount { role: r.role, count: r.count }).collect();

    let series = sqlx::query!(
        r#"SELECT s.id AS "series_id!: i64", s.title AS "name!", s.cover_url,
                  COUNT(DISTINCT i.id) AS "issue_count!: i64"
           FROM issue_credits ic JOIN issues i ON i.id = ic.issue_id JOIN series s ON s.id = i.series_id
           WHERE ic.creator_id = ?
             AND EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id AND f.status='owned' AND f.is_present=1)
           GROUP BY s.id ORDER BY issue_count DESC"#, id
    ).fetch_all(&*pool).await?
     .into_iter().map(|r| CreatorSeries {
        series_id: r.series_id, name: r.name, issue_count: r.issue_count, cover_url: r.cover_url,
     }).collect();

    Ok(Some(CreatorDetail { id: base.id, name: base.name, cv_person_id: base.cv_person_id, roles, series }))
}

/// Paginated in-library issues for a creator, optional role + series filters.
/// Page is 1-based; page size 50; ordered by cover_date ASC.
pub async fn creator_issues<'e, E>(
    executor: E, creator_id: i64, role: Option<&str>, series_id: Option<i64>, page: i64,
) -> Result<Vec<CreatorIssueRow>>
where E: SqliteExecutor<'e> {
    let offset = (page.max(1) - 1) * 50;
    let rows = sqlx::query!(
        r#"SELECT i.id AS "issue_id!: i64", s.title AS "series_name!",
                  i.number AS "issue_number!", i.cover_date, i.cover_url, ic.role AS "role!"
           FROM issue_credits ic JOIN issues i ON i.id = ic.issue_id JOIN series s ON s.id = i.series_id
           WHERE ic.creator_id = ?
             AND EXISTS (SELECT 1 FROM files f WHERE f.issue_id=i.id AND f.status='owned' AND f.is_present=1)
             AND (? IS NULL OR ic.role = ?)
             AND (? IS NULL OR i.series_id = ?)
           ORDER BY i.cover_date ASC
           LIMIT 50 OFFSET ?"#,
        creator_id, role, role, series_id, series_id, offset,
    ).fetch_all(executor).await?;
    Ok(rows.into_iter().map(|r| CreatorIssueRow {
        issue_id: r.issue_id, series_name: r.series_name, issue_number: r.issue_number,
        cover_date: r.cover_date, cover_url: r.cover_url, role: r.role,
    }).collect())
}
```

- [ ] **Step 4: sqlx prepare + run**

```bash
export DATABASE_URL="sqlite:/tmp/lb-credits-prepare.db?mode=rwc"
cargo sqlx prepare --workspace
SQLX_OFFLINE=true cargo test -p longbox-db --test creators
```
Expected: all creators tests PASS.

- [ ] **Step 5: Commit**

```bash
git add longbox-db/src/creator_repo.rs longbox-db/tests/creators.rs .sqlx
git commit -m "feat(db): creator search/detail/issues read queries"
```

### Task 8: HTTP handlers

**Files:** Create `longbox-web/src/routes/creators.rs`; modify `longbox-web/src/routes/mod.rs`

- [ ] **Step 1: Create `longbox-web/src/routes/creators.rs`** (mirror `cv_search.rs`'s `State`/`Query`/`Json`/`ApiError` shape)

```rust
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use longbox_db::creator_repo::{self, CreatorDetail, CreatorIssueRow, CreatorSearchRow};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/creators/search", get(search))
        .route("/creators/:id", get(detail))
        .route("/creators/:id/issues", get(issues))
}

#[derive(Debug, Deserialize)]
struct SearchParams { q: String }

async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<CreatorSearchRow>>, ApiError> {
    let q = params.q.trim();
    if q.len() < 2 {
        return Err(ApiError::BadRequest { message: "query `q` must be at least 2 characters".into() });
    }
    Ok(Json(creator_repo::search_creators(&state.db, q).await?))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<CreatorDetail>, ApiError> {
    creator_repo::creator_detail(&state.db, id).await?
        .map(Json)
        .ok_or(ApiError::NotFound { resource: "creator", id: id.to_string() })
}

#[derive(Debug, Deserialize)]
struct IssuesParams {
    role: Option<String>,
    series_id: Option<i64>,
    #[serde(default = "one")]
    page: i64,
}
fn one() -> i64 { 1 }

async fn issues(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(p): Query<IssuesParams>,
) -> Result<Json<Vec<CreatorIssueRow>>, ApiError> {
    Ok(Json(creator_repo::creator_issues(
        &state.db, id, p.role.as_deref(), p.series_id, p.page,
    ).await?))
}
```

> Verify the exact `ApiError::NotFound` field names against `longbox-web/src/error.rs` (the map showed `NotFound { resource, id }`); match them. `resource` is `&'static str` or `String` per the enum — adjust the literal accordingly.

- [ ] **Step 2: Register** in `longbox-web/src/routes/mod.rs` — add `pub mod creators;` in the module block and `.merge(creators::router())` in the `/api` router chain (next to `.merge(cv_search::router())`).

- [ ] **Step 3: Build + run web tests**

Run: `SQLX_OFFLINE=true cargo test -p longbox-web`
Expected: compiles; existing web tests still green. (If `longbox-web/tests/api_tests.rs` has a harness like `build_test_app`, add a smoke test: seed a creator+owned issue, GET `/api/creators/search?q=…`, assert 200 + one row. Mirror an existing api_test; if the harness is heavy, the repo-layer tests already cover query correctness — a handler smoke test is sufficient, don't rebuild the world.)

- [ ] **Step 4: Commit**

```bash
git add longbox-web/src/routes/creators.rs longbox-web/src/routes/mod.rs
git commit -m "feat(web): /api/creators search, detail, issues endpoints"
```

- [ ] **Step 5: REVIEWER GATE** — run `superpowers:code-reviewer` over Commit 5 (read queries + handlers). Focus: owned-only predicate applied consistently across all three queries + the search/detail counts; the `(? IS NULL OR col = ?)` optional-filter binding; SQL injection surface (all params are bound, `LIKE` uses a bound `%…%`); pagination bounds. Address findings before the frontend.

---

## Commit 6 — Frontend

### Task 9: API client module

**Files:** Create `longbox-frontend/src/lib/api/creators.ts`

- [ ] **Step 1: Create the module** (mirror `$lib/api/series.ts` using `apiFetch`)

```ts
import { apiFetch } from './client';

export interface CreatorSearchRow {
  id: number; name: string; cv_person_id: number | null;
  series_count: number; issue_count: number;
}
export interface RoleCount { role: string; count: number }
export interface CreatorSeries { series_id: number; name: string; issue_count: number; cover_url: string | null }
export interface CreatorDetail {
  id: number; name: string; cv_person_id: number | null;
  roles: RoleCount[]; series: CreatorSeries[];
}
export interface CreatorIssueRow {
  issue_id: number; series_name: string; issue_number: string;
  cover_date: string | null; cover_url: string | null; role: string;
}

export function searchCreators(q: string): Promise<CreatorSearchRow[]> {
  return apiFetch(`/creators/search?q=${encodeURIComponent(q)}`);
}
export function getCreator(id: number): Promise<CreatorDetail> {
  return apiFetch(`/creators/${id}`);
}
export function getCreatorIssues(
  id: number, opts: { role?: string; series_id?: number; page?: number } = {},
): Promise<CreatorIssueRow[]> {
  const p = new URLSearchParams();
  if (opts.role) p.set('role', opts.role);
  if (opts.series_id != null) p.set('series_id', String(opts.series_id));
  if (opts.page != null) p.set('page', String(opts.page));
  const qs = p.toString();
  return apiFetch(`/creators/${id}/issues${qs ? `?${qs}` : ''}`);
}
```

- [ ] **Step 2: Commit**

```bash
git add longbox-frontend/src/lib/api/creators.ts
git commit -m "feat(frontend): creators API client"
```

### Task 10: Search page + nav

**Files:** Create `longbox-frontend/src/routes/creators/+page.svelte` and `+page.ts`; modify `longbox-frontend/src/lib/components/NavBar.svelte`

- [ ] **Step 1: Create `+page.ts`** (no server load — search is client-driven)

```ts
export const load = async () => ({});
```

- [ ] **Step 2: Create `+page.svelte`** (debounced search; mirror existing page markup/styling conventions in `routes/series/+page.svelte`)

```svelte
<script lang="ts">
  import { searchCreators, type CreatorSearchRow } from '$lib/api/creators';
  let q = '';
  let results: CreatorSearchRow[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  let loading = false;

  function onInput() {
    clearTimeout(timer);
    const term = q.trim();
    if (term.length < 2) { results = []; return; }
    timer = setTimeout(async () => {
      loading = true;
      try { results = await searchCreators(term); }
      finally { loading = false; }
    }, 300);
  }
</script>

<h1>Creators</h1>
<input type="search" placeholder="Search creators…" bind:value={q} on:input={onInput} />
{#if loading}<p>Searching…</p>{/if}
<ul>
  {#each results as c (c.id)}
    <li><a href={`/creators/${c.id}`}>{c.name}</a>
        <span>{c.issue_count} issues · {c.series_count} series</span></li>
  {/each}
</ul>
```

- [ ] **Step 3: Add the nav item** in `longbox-frontend/src/lib/components/NavBar.svelte` — add to the `nav: NavItem[]` array a top-level link (per decision #6), e.g. after the `Library` menu:

```ts
  { kind: 'link', href: '/creators', label: 'Creators' },
```

- [ ] **Step 4: Verify the frontend builds**

Run: `cd longbox-frontend && npm run build` (or the repo's check script, e.g. `npm run check`)
Expected: builds clean; `/creators` route compiles. (`isActive` already handles `/creators/[id]` via its `startsWith(href + '/')` check.)

- [ ] **Step 5: Commit**

```bash
git add longbox-frontend/src/routes/creators/+page.svelte longbox-frontend/src/routes/creators/+page.ts longbox-frontend/src/lib/components/NavBar.svelte
git commit -m "feat(frontend): creator search page + nav"
```

### Task 11: Creator detail page

**Files:** Create `longbox-frontend/src/routes/creators/[id]/+page.svelte` and `+page.ts`

- [ ] **Step 1: Create `+page.ts`** (load creator detail by route param)

```ts
import { getCreator } from '$lib/api/creators';

export const load = async ({ params }) => {
  const creator = await getCreator(Number(params.id));
  return { creator };
};
```

- [ ] **Step 2: Create `+page.svelte`** (name heading, role chips with counts, series list linking to existing series detail)

```svelte
<script lang="ts">
  export let data;
  $: creator = data.creator;
</script>

<h1>{creator.name}</h1>

<div class="roles">
  {#each creator.roles as r (r.role)}
    <span class="chip">{r.role} · {r.count}</span>
  {/each}
</div>

<ul class="series">
  {#each creator.series as s (s.series_id)}
    <li>
      <a href={`/series/${s.series_id}`}>
        {#if s.cover_url}<img src={s.cover_url} alt="" width="40" />{/if}
        {s.name}
      </a>
      <span>{s.issue_count} issues</span>
    </li>
  {/each}
</ul>
```

- [ ] **Step 3: Verify build + commit**

Run: `cd longbox-frontend && npm run build`
Expected: clean.

```bash
git add longbox-frontend/src/routes/creators/\[id\]/+page.svelte longbox-frontend/src/routes/creators/\[id\]/+page.ts
git commit -m "feat(frontend): creator detail page"
```

---

## Final verification

- [ ] `SQLX_OFFLINE=true cargo test --workspace` — all green.
- [ ] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets` — no NEW warnings vs the branch point.
- [ ] `cd longbox-frontend && npm run build && npm run check` — clean.
- [ ] **Live smoke (post-deploy):** with the new binary running, confirm the migration applied, then watch the resolver populate credits — `tracing` target `longbox_credits` should log "credits resolver pass" with a nonzero `processed`. After a pass, `GET /api/creators/search?q=remender` returns a creator with owned issue/series counts, and `/creators` → click → detail shows role chips + series.

## Out of scope (do not build here)
- Creator Discovery (non-owned work via CV `volume_credits`) — separate feature.
- Metron credits — hook point exists (`MetronIssueDetailRow` drops `credits`); wire later.
- Live CV person search on the search page (local DB only).
- Role canonical grouping / re-fetch path / inline `commit_merge` credit fetch.

## Self-review notes
- **Spec coverage:** schema → Task 1; CV DTO+fetch → Tasks 2–3; persist → Task 4; resolver (replaces old Commit 4 inline hook + Commit 5 nightly) → Tasks 5–6; read API → Tasks 7–8; frontend → Tasks 9–11. All 8 locked decisions implemented (atomic-role split T2; defer-to-resolver T6 + DEFAULT 0; continuous resolver T6; owned+cv_id-not-null scope T5; ON CONFLICT name T4; top-nav T10; no re-fetch — by omission; raw roles T7/T11).
- **Type consistency:** `CvPersonCredit { cv_person_id, name, role }` flows T2→T3 (client return) →T4/T6 (`insert_issue_credits`/resolver). `IssueNeedingCredits { issue_id, cv_issue_id }` T5→T6. `insert_issue_credits(&Pool, i64, &[CvPersonCredit])` manages its own tx; `upsert_creator(executor,…)` is generic so it composes inside that tx. Read structs (`CreatorSearchRow`/`CreatorDetail`/`RoleCount`/`CreatorSeries`/`CreatorIssueRow`) defined T7, consumed by handlers T8 and TS interfaces T9.
- **Deviations from spec (intentional):** one migration file (3 DDL statements) not three files; `fetch_issue_credits` returns `Vec<CvPersonCredit>` rather than overloading the bulk-list `CvIssueDetail`; old Commit 4 inline hook dropped per decision #2; backfill is continuous (decision #3) not the 2am batch; CV budget is 180/h (not 200) so backfill ≈ 51h cumulative.
