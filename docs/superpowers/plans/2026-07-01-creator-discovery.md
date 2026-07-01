# Creator Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From a creator's detail page, show their full ComicVine series bibliography (via the Person resource's `volume_credits`), split into "in your library" vs "not in your library", and let the user acquire a not-owned series with one click.

**Architecture:** A new CV client method `fetch_person_volume_credits` (reverse lookup: person → the series they're credited on) feeds a `GET /api/creators/:id/discover` endpoint that joins the CV volume ids against the local `series.cv_id` catalog. The frontend adds a button-triggered "Discover" section to the existing creator detail page; the "acquire" button reuses the existing `POST /api/series {cv_id}` add flow (which already does fetch-volume + fetch-issues + insert + auto-pull-search). No schema, no cache (one CV call per view), no roles (CV `volume_credits` is series-level only) — all locked kickoff decisions.

**Tech Stack:** Rust, Axum 0.7, sqlx (SQLite, offline metadata), ComicVine API, SvelteKit (Svelte 5 runes).

**Locked kickoff decisions:**
1. **Scope = already-ingested creators only** (a "Discover" action on the existing `/creators/:id` page; no arbitrary CV person-search UI — that's a fast-follow).
2. **Minimal display** — volume name + owned badge + acquire button; one CV call per view (no per-volume detail enrichment — infeasible for 800–1,200-volume creators).
3. Layout: split "in your library" / "not in your library", alphabetical, client-side (bounded list ≤~1,500, no server pagination).
4. **Live fetch, no cache** (one call; add a cache only if the rate-limit chip shows it hurts).
5. Show all volumes (no noise filtering — we only have id+name, can't cheaply filter).
6. No role attribution in discovery (CV `volume_credits` carries no role).
7. Acquire = reuse `POST /api/series {cv_id}` (no new endpoint; inherits folder/rematch/auto-pull-search).

**CI is enforced now** (added since Creator Credits): every commit must pass `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. **Run `cargo fmt` before each commit** or CI goes red. Use `SQLX_OFFLINE=true` for cargo commands.

**Pre-flight:**
```bash
git checkout -b feat/creator-discovery
SQLX_OFFLINE=true cargo test --workspace   # baseline green
export DATABASE_URL="sqlite:/tmp/lb-discovery-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
```

**Reviewer gate** (superpowers:code-reviewer) after **Commit 3** (the CV integration + owned-join endpoint — the substantive/risky part). Frontend (Commit 4) is low-risk, self-review only.

---

## Investigation facts (verified, so the implementer doesn't re-derive)

- **CV `volume_credits`** (live-verified): `/person/4040-<id>/?field_list=volume_credits` returns the COMPLETE list (Remender 325, Bendis 794, Stan Lee 1231 — not truncated), each entry `{id, name, api_detail_url, site_detail_url}`. We use only `id` + `name`.
- **CV client pattern** — `fetch_volume` (`longbox-comicvine/src/client.rs:198`) is the exact template: `build_url(path, &[params])` → `execute_with_retry(url)` → `parse_envelope::<Raw>(&body)` → `unwrap_envelope_results(env, &body)` → `project_*`. Person resource prefix is `4040-` (volume is `4050-`, issue is `4000-`).
- **`state.cv`** is the interactive CV client (used by `cv_search.rs`'s handler: `state.cv.search_volumes(...)`). Discovery uses `state.cv.fetch_person_volume_credits(...)`.
- **Acquire reuse** — `POST /api/series {cv_id: <volume id>}` → `add_or_get_from_cv` (`longbox-web/src/routes/series.rs:306`); frontend `addSeries(cvId)` (`longbox-frontend/src/lib/api/series.ts:21`). Idempotent (409 on dupe).
- **Owned join** — `series.cv_id` IS the CV volume id. There's no batch lookup yet; this plan adds `series_repo::existing_cv_id_pairs`.
- **Entry point** — `cv_person_id` is already on `CreatorDetail` → frontend `data.creator.cv_person_id`, and the detail page (`creators/[id]/+page.svelte`, Svelte 5 runes) currently renders only roles + series.
- **Add UI to mirror** — `longbox-frontend/src/routes/add/+page.svelte` `handleAdd(cvId, name)` + per-row `addingId`/`addedIds` button state.

---

## File Structure

| File | Responsibility | Commit |
|------|----------------|--------|
| `longbox-comicvine/src/models.rs` (modify) | raw `CvVolumeCreditRaw` + `CvPersonVolumeCreditsRaw` | 1 |
| `longbox-comicvine/src/projection.rs` (modify) | public `CvVolumeCredit` + `project_person_volume_credits` (dedup) | 1 |
| `longbox-comicvine/src/client.rs` (modify) | `fetch_person_volume_credits` | 1 |
| `longbox-comicvine/src/lib.rs` (modify) | re-export `CvVolumeCredit` | 1 |
| `longbox-db/src/series_repo.rs` (modify) | `existing_cv_id_pairs` | 2 |
| `longbox-db/src/creator_repo.rs` (modify) | `cv_person_id_of` | 2 |
| `longbox-db/tests/series.rs`, `tests/creators.rs` (modify) | repo tests | 2 |
| `longbox-web/src/routes/creators.rs` (modify) | `GET /creators/:id/discover` + `DiscoveredVolume` + `build_discovery` | 3 |
| `longbox-web/tests/api_tests.rs` (modify) | endpoint smoke test | 3 |
| `longbox-frontend/src/lib/api/creators.ts` (modify) | `DiscoveredVolume` + `getCreatorDiscovery` | 4 |
| `longbox-frontend/src/routes/creators/[id]/+page.svelte` (modify) | Discover section (button, split list, acquire) | 4 |

---

## Commit 1 — CV person → volume_credits

### Task 1: DTO + dedup projection

**Files:** Modify `longbox-comicvine/src/models.rs`, `longbox-comicvine/src/projection.rs`

- [ ] **Step 1: Write the failing test** — add to `projection.rs` (new `#[cfg(test)] mod volume_credit_tests`)

```rust
#[cfg(test)]
mod volume_credit_tests {
    use super::*;
    use crate::models::{CvPersonVolumeCreditsRaw, CvVolumeCreditRaw};

    #[test]
    fn projects_and_dedupes_by_volume_id() {
        let raw = CvPersonVolumeCreditsRaw {
            volume_credits: vec![
                CvVolumeCreditRaw { id: 7084, name: "Avengers".into() },
                CvVolumeCreditRaw { id: 18166, name: "Uncanny X-Force".into() },
                CvVolumeCreditRaw { id: 7084, name: "Avengers".into() }, // dup -> dropped
            ],
        };
        let out = project_person_volume_credits(raw);
        assert_eq!(out, vec![
            CvVolumeCredit { cv_volume_id: 7084, name: "Avengers".into() },
            CvVolumeCredit { cv_volume_id: 18166, name: "Uncanny X-Force".into() },
        ]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-comicvine projects_and_dedupes`
Expected: FAIL to compile (types/fn undefined).

- [ ] **Step 3: Add raw DTOs** in `longbox-comicvine/src/models.rs` (near `CvIssueCreditsRaw`)

```rust
/// One entry of a person's `volume_credits` — a series they're credited on.
/// From `/person/4040-<id>/?field_list=volume_credits`. Only id+name used.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvVolumeCreditRaw {
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

/// The `results` object of a person volume-credits fetch (field_list-limited).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvPersonVolumeCreditsRaw {
    #[serde(default)]
    pub volume_credits: Vec<CvVolumeCreditRaw>,
}
```

- [ ] **Step 4: Add public type + projection** in `longbox-comicvine/src/projection.rs` (add `CvPersonVolumeCreditsRaw` to the existing `use crate::models::{...}` line)

```rust
/// One series (CV volume) a creator is credited on — a bibliography entry.
/// Series-level only: CV's `volume_credits` carries no role/issue detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvVolumeCredit {
    pub cv_volume_id: i64,
    pub name: String,
}

/// Project a person's raw volume_credits into deduped `CvVolumeCredit` entries
/// (CV occasionally repeats a volume; keep first occurrence, preserve order).
pub(crate) fn project_person_volume_credits(raw: CvPersonVolumeCreditsRaw) -> Vec<CvVolumeCredit> {
    let mut seen = std::collections::HashSet::new();
    raw.volume_credits
        .into_iter()
        .filter(|v| seen.insert(v.id))
        .map(|v| CvVolumeCredit { cv_volume_id: v.id, name: v.name })
        .collect()
}
```

- [ ] **Step 5: Run to verify pass**

Run: `SQLX_OFFLINE=true cargo test -p longbox-comicvine projects_and_dedupes`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add longbox-comicvine/src/models.rs longbox-comicvine/src/projection.rs
git commit -m "feat(comicvine): person volume_credits DTO + dedup projection"
```

### Task 2: `fetch_person_volume_credits` client method

**Files:** Modify `longbox-comicvine/src/client.rs`, `longbox-comicvine/src/lib.rs`

- [ ] **Step 1: Add the client method** in `impl ComicVineClient` (after `fetch_issue_credits`), mirroring `fetch_volume`'s helper chain

```rust
/// Fetch the series (volumes) a person is credited on — their full
/// bibliography, CV's Person `volume_credits` reverse lookup. Series-level
/// only. Returns the complete list in one call (CV does not paginate this).
#[instrument(target = "longbox_comicvine", skip(self))]
pub async fn fetch_person_volume_credits(
    &self,
    cv_person_id: i64,
) -> Result<Vec<CvVolumeCredit>, CvError> {
    let path = format!("person/4040-{cv_person_id}/");
    let url = self.build_url(&path, &[("field_list", "volume_credits")])?;
    let body = self.execute_with_retry(url).await?;
    let envelope = parse_envelope::<CvPersonVolumeCreditsRaw>(&body)?;
    let raw = unwrap_envelope_results(envelope, &body)?;
    Ok(project_person_volume_credits(raw))
}
```

Ensure `CvVolumeCredit`, `project_person_volume_credits` (from `crate::projection`) and `CvPersonVolumeCreditsRaw` (from `crate::models`) are in the client.rs imports — add them alongside the existing `CvIssueCreditsRaw`/`project_issue_credits` imports.

- [ ] **Step 2: Re-export the public type** in `longbox-comicvine/src/lib.rs` — add `CvVolumeCredit` to the `pub use projection::{...}` line (next to `CvPersonCredit`).

- [ ] **Step 3: Build (HTTP method — exercised by the endpoint in Commit 3)**

Run: `SQLX_OFFLINE=true cargo build -p longbox-comicvine` and `SQLX_OFFLINE=true cargo clippy -p longbox-comicvine --all-targets -- -D warnings`
Expected: clean; `CvVolumeCredit` resolves as `longbox_comicvine::CvVolumeCredit`.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add longbox-comicvine/src/client.rs longbox-comicvine/src/lib.rs
git commit -m "feat(comicvine): fetch_person_volume_credits (bibliography reverse lookup)"
```

---

## Commit 2 — DB helpers (owned join + creator person id)

### Task 3: `series_repo::existing_cv_id_pairs` + `creator_repo::cv_person_id_of`

**Files:** Modify `longbox-db/src/series_repo.rs`, `longbox-db/src/creator_repo.rs`, `longbox-db/tests/series.rs`, `longbox-db/tests/creators.rs`

- [ ] **Step 1: Write the failing tests**

Add to `longbox-db/tests/series.rs` (reuses the file's `fresh_pool`/`walking_dead` helpers):

```rust
#[tokio::test]
async fn existing_cv_id_pairs_returns_id_and_cv_id() {
    let pool = fresh_pool().await;
    let s = series_repo::insert(&pool, walking_dead()).await.unwrap(); // cv_id 12345
    // a series with no cv_id must be excluded
    series_repo::insert(&pool, NewSeries { cv_id: None, ..walking_dead() }).await.unwrap();
    let pairs = series_repo::existing_cv_id_pairs(&pool).await.unwrap();
    assert_eq!(pairs, vec![(s.id, 12345)]);
}
```

Add to `longbox-db/tests/creators.rs` (reuses `fresh_pool`; `insert_issue_credits` upserts a creator with a cv_person_id):

```rust
#[tokio::test]
async fn cv_person_id_of_returns_person_id_or_none() {
    use longbox_comicvine::CvPersonCredit;
    let pool = fresh_pool().await;
    let iid = seed_owned_issue(&pool, 5001).await;
    creator_repo::insert_issue_credits(&pool, iid, &[
        CvPersonCredit { cv_person_id: 55, name: "Rick Remender".into(), role: "writer".into() },
    ]).await.unwrap();
    // find the creator id via search
    let hits = creator_repo::search_creators(&pool, "remender").await.unwrap();
    let cid = hits[0].id;
    assert_eq!(creator_repo::cv_person_id_of(&pool, cid).await.unwrap(), Some(55));
    // unknown creator -> None
    assert_eq!(creator_repo::cv_person_id_of(&pool, 999999).await.unwrap(), None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db existing_cv_id_pairs cv_person_id_of`
Expected: FAIL (undefined functions).

- [ ] **Step 3: Implement** — add to `longbox-db/src/series_repo.rs`

```rust
/// `(local series id, cv_id)` for every catalog series that has a CV volume
/// id. Discovery joins a creator's `volume_credits` against this to split
/// in-library vs not, and to link owned volumes to their local series row.
pub async fn existing_cv_id_pairs<'e, E>(executor: E) -> Result<Vec<(i64, i64)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", cv_id AS "cv_id!: i64"
           FROM series WHERE cv_id IS NOT NULL"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.cv_id)).collect())
}
```

Add to `longbox-db/src/creator_repo.rs`:

```rust
/// The CV person id for a creator. `None` when the creator doesn't exist OR
/// has no `cv_person_id` — both mean "no discovery possible" to the caller.
pub async fn cv_person_id_of<'e, E>(executor: E, creator_id: i64) -> Result<Option<i64>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(
        r#"SELECT cv_person_id FROM creators WHERE id = ?"#,
        creator_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.and_then(|r| r.cv_person_id))
}
```

- [ ] **Step 4: Regenerate sqlx metadata + run**

```bash
export DATABASE_URL="sqlite:/tmp/lb-discovery-prepare.db?mode=rwc"
cargo sqlx prepare --workspace -- --all-targets
SQLX_OFFLINE=true cargo test -p longbox-db existing_cv_id_pairs cv_person_id_of
```
Expected: both tests PASS; `git diff --stat .sqlx/` adds 2 new query json files, no unrelated deletions.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add longbox-db/src/series_repo.rs longbox-db/src/creator_repo.rs longbox-db/tests/series.rs longbox-db/tests/creators.rs .sqlx
git commit -m "feat(db): existing_cv_id_pairs + cv_person_id_of for discovery"
```

---

## Commit 3 — Discovery endpoint  ⟶ REVIEWER GATE after this commit

### Task 4: `GET /api/creators/:id/discover`

**Files:** Modify `longbox-web/src/routes/creators.rs`, `longbox-web/tests/api_tests.rs`

- [ ] **Step 1: Write the failing test for the pure join/sort helper** — add to `longbox-web/src/routes/creators.rs` (a `#[cfg(test)] mod discover_tests`)

```rust
#[cfg(test)]
mod discover_tests {
    use super::*;
    use longbox_comicvine::CvVolumeCredit;

    #[test]
    fn build_discovery_maps_owned_and_sorts_case_insensitive() {
        let credits = vec![
            CvVolumeCredit { cv_volume_id: 7084, name: "avengers".into() },
            CvVolumeCredit { cv_volume_id: 999, name: "Deadly Class".into() },
        ];
        // series id 3 owns cv volume 7084; 999 is not in the library
        let owned = vec![(3_i64, 7084_i64)];
        let out = build_discovery(credits, &owned);
        assert_eq!(out, vec![
            DiscoveredVolume { cv_volume_id: 7084, name: "avengers".into(), series_id: Some(3) },
            DiscoveredVolume { cv_volume_id: 999, name: "Deadly Class".into(), series_id: None },
        ]);
        // case-insensitive sort put lowercase "avengers" before "Deadly Class"
        assert_eq!(out[0].cv_volume_id, 7084);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-web build_discovery_maps_owned`
Expected: FAIL (undefined `build_discovery`/`DiscoveredVolume`).

- [ ] **Step 3: Implement the type, helper, handler, and route** in `longbox-web/src/routes/creators.rs`

Add imports at the top (join the existing `use` blocks): `use longbox_comicvine::CvVolumeCredit;`, `use longbox_db::{creator_repo, series_repo};` (extend the existing `creator_repo` import), and `use std::collections::HashMap;`.

Add the route to `router()` (next to the other `/creators/...` routes):

```rust
        .route("/creators/:id/discover", get(discover))
```

Add the type + pure helper + handler:

```rust
/// One series in a creator's CV bibliography. `series_id` is `Some(local id)`
/// when the volume is already in the library (link to it), `None` when not
/// (offer to acquire via `POST /api/series {cv_id}`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct DiscoveredVolume {
    cv_volume_id: i64,
    name: String,
    series_id: Option<i64>,
}

/// Pure join+sort: map each CV volume credit to owned/not-owned against the
/// catalog's `(series_id, cv_id)` pairs, then sort by name case-insensitively.
fn build_discovery(credits: Vec<CvVolumeCredit>, owned_pairs: &[(i64, i64)]) -> Vec<DiscoveredVolume> {
    let owned: HashMap<i64, i64> = owned_pairs.iter().map(|(sid, cvid)| (*cvid, *sid)).collect();
    let mut out: Vec<DiscoveredVolume> = credits
        .into_iter()
        .map(|c| DiscoveredVolume {
            series_id: owned.get(&c.cv_volume_id).copied(),
            cv_volume_id: c.cv_volume_id,
            name: c.name,
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// A creator's full CV series bibliography, owned/not-owned flagged. Live CV
/// call (one request); empty when the creator has no known cv_person_id.
async fn discover(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<DiscoveredVolume>>, ApiError> {
    let Some(person_id) = creator_repo::cv_person_id_of(&state.db, id).await? else {
        return Ok(Json(Vec::new()));
    };
    let credits = state.cv.fetch_person_volume_credits(person_id).await?;
    let owned = series_repo::existing_cv_id_pairs(&state.db).await?;
    Ok(Json(build_discovery(credits, &owned)))
}
```

Note: `state.cv` is the interactive CV client (same one `cv_search`'s handler uses). If the codebase convention for on-demand user fetches is a differently-named field, match `cv_search.rs`'s handler.

- [ ] **Step 4: Add an endpoint smoke test** in `longbox-web/tests/api_tests.rs` — the no-cv-person-id path returns 200 `[]` WITHOUT hitting CV (so it's network-free and safe in CI). Mirror the existing `creators_*` test harness; seed a creator row with `cv_person_id = NULL` directly (or via a helper), then:

```rust
#[tokio::test]
async fn creators_discover_empty_when_no_cv_person_id() {
    let app = build_test_app().await;
    // Insert a creator with NULL cv_person_id directly (no ingestion needed).
    let cid: i64 = sqlx::query_scalar(
        "INSERT INTO creators (name, cv_person_id) VALUES ('Nobody', NULL) RETURNING id",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let resp = app
        .request(empty_request("GET", &format!("/api/creators/{cid}/discover")))
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(response_json(resp).await.as_array().unwrap().len(), 0);
}
```

> Match the real harness helpers (`build_test_app`/`app.request`/`empty_request`/`response_json`) used by the existing `creators_*` tests — read one of them (e.g. `creators_detail_missing_id_returns_404`) and mirror its exact shape. Do NOT stand up a new harness. If seeding via raw SQL doesn't fit the harness, use whatever creator-insert helper the neighbouring tests use, setting `cv_person_id` to NULL.

- [ ] **Step 5: Run to verify pass**

```bash
SQLX_OFFLINE=true cargo test -p longbox-web build_discovery_maps_owned creators_discover_empty
SQLX_OFFLINE=true cargo clippy -p longbox-web --all-targets -- -D warnings
```
Expected: both tests PASS, clippy clean.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add longbox-web/src/routes/creators.rs longbox-web/tests/api_tests.rs
git commit -m "feat(web): GET /api/creators/:id/discover (bibliography, owned-flagged)"
```

- [ ] **Step 7: REVIEWER GATE** — run `superpowers:code-reviewer` over Commits 1–3. Focus: the CV method mirrors the client pattern correctly; the owned-join maps volume→series correctly (and `series_id` links owned rows); the no-cv-person-id path can't hit CV; live CV call on `state.cv` is the right client (won't be spammed — it's one user-initiated call); sort/dedup correctness. Address findings before the frontend.

---

## Commit 4 — Frontend Discover section

### Task 5: API client

**Files:** Modify `longbox-frontend/src/lib/api/creators.ts`

- [ ] **Step 1: Add the type + fetch** (append to the existing module)

```ts
export interface DiscoveredVolume {
  cv_volume_id: number;
  name: string;
  series_id: number | null; // non-null => already in the library
}

export function getCreatorDiscovery(id: number): Promise<DiscoveredVolume[]> {
  return apiFetch(`/creators/${id}/discover`);
}
```

- [ ] **Step 2: Commit**

```bash
git add longbox-frontend/src/lib/api/creators.ts
git commit -m "feat(frontend): creator discovery API client"
```

### Task 6: Discover section on the creator detail page

**Files:** Modify `longbox-frontend/src/routes/creators/[id]/+page.svelte`

- [ ] **Step 1: Read the existing file** to see its current `<script>` (`let { data } = $props()`, `$derived` creator) and markup (name heading, role chips, series list). You are ADDING a Discover section below the existing content, reusing the acquire pattern from `routes/add/+page.svelte`.

- [ ] **Step 2: Extend the `<script>`** — add discovery state + button-triggered load + acquire (button-triggered, not auto-load: the CV call can be slow for prolific creators). Add these to the existing `<script lang="ts">`:

```ts
  import { getCreatorDiscovery, type DiscoveredVolume } from '$lib/api/creators';
  import { addSeries } from '$lib/api/series';

  let discovery = $state<DiscoveredVolume[] | null>(null);
  let discovering = $state(false);
  let discoverError = $state<string | null>(null);
  let addingId = $state<number | null>(null);
  let addedIds = $state<Set<number>>(new Set());

  async function loadDiscovery() {
    discovering = true;
    discoverError = null;
    try {
      discovery = await getCreatorDiscovery(data.creator.id);
    } catch (e) {
      discoverError = e instanceof Error ? e.message : 'Failed to load bibliography';
    } finally {
      discovering = false;
    }
  }

  async function acquire(cvVolumeId: number) {
    addingId = cvVolumeId;
    try {
      await addSeries(cvVolumeId);
      addedIds = new Set(addedIds).add(cvVolumeId);
    } finally {
      addingId = null;
    }
  }

  const inLibrary = $derived((discovery ?? []).filter((d) => d.series_id !== null));
  const notInLibrary = $derived((discovery ?? []).filter((d) => d.series_id === null));
```

- [ ] **Step 3: Add the markup** below the existing series list

```svelte
<section class="mt-8">
  {#if discovery === null}
    <button
      class="rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium hover:bg-slate-50 disabled:opacity-50"
      onclick={loadDiscovery}
      disabled={discovering}
    >
      {discovering ? 'Loading bibliography…' : `Discover more by ${data.creator.name}`}
    </button>
    {#if discoverError}<p class="mt-2 text-sm text-red-600">{discoverError}</p>{/if}
  {:else}
    <h2 class="mb-2 text-lg font-semibold">Not in your library ({notInLibrary.length})</h2>
    <ul class="mb-6 space-y-1">
      {#each notInLibrary as v (v.cv_volume_id)}
        <li class="flex items-baseline justify-between gap-2">
          <span>{v.name}</span>
          {#if addedIds.has(v.cv_volume_id)}
            <span class="text-sm text-green-600">✓ Added</span>
          {:else}
            <button
              class="rounded border border-slate-300 px-2 py-0.5 text-sm hover:bg-slate-50 disabled:opacity-50"
              onclick={() => acquire(v.cv_volume_id)}
              disabled={addingId === v.cv_volume_id}
            >
              {addingId === v.cv_volume_id ? 'Adding…' : 'Add to Library'}
            </button>
          {/if}
        </li>
      {/each}
    </ul>

    <h2 class="mb-2 text-lg font-semibold">In your library ({inLibrary.length})</h2>
    <ul class="space-y-1">
      {#each inLibrary as v (v.cv_volume_id)}
        <li><a href={`/series/${v.series_id}`} class="hover:underline">{v.name}</a></li>
      {/each}
    </ul>
  {/if}
</section>
```

> Match the existing page's Tailwind class conventions and the `Button` component if the page/`add` page uses a shared `<Button>` rather than raw `<button>` — mirror whichever `routes/add/+page.svelte` uses for the acquire button. Keep the runes style (`$state`/`$derived`/`onclick`) consistent with the existing file.

- [ ] **Step 4: Verify build**

Run: `cd longbox-frontend && pnpm build`
Expected: clean. (`pnpm check`/`pnpm test` have pre-existing failures in unrelated files — don't gate on them; just confirm `pnpm build` is green and no NEW errors reference `creators/[id]`.)

- [ ] **Step 5: Commit**

```bash
git add longbox-frontend/src/routes/creators/\[id\]/+page.svelte
git commit -m "feat(frontend): Discover section on creator detail page"
```

---

## Final verification

- [ ] `SQLX_OFFLINE=true cargo test --workspace` — green.
- [ ] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings` — clean (CI gate).
- [ ] `cargo fmt --all -- --check` — clean (CI gate).
- [ ] `cd longbox-frontend && pnpm build` — clean.
- [ ] **Live smoke (post-deploy):** open a creator that has owned issues (e.g. one the resolver has ingested), click "Discover more by …", confirm the bibliography loads (one CV call), owned volumes appear under "In your library" linking to `/series/:id`, and "Add to Library" on a not-owned volume creates the series (the existing add flow: folder + auto-pull-search fire).

## Out of scope (do not build here)
- Arbitrary CV person-search UI (discover creators you own nothing by) — fast-follow (CV person search confirmed working).
- Per-volume enrichment (cover/year/publisher/issue-count) — infeasible at 800–1,200 volumes/creator; would need a background cache pass.
- Caching `volume_credits` — live per view for v1.
- Role-filtered discovery — CV `volume_credits` has no roles.
- Metron discovery.

## Self-review notes
- **Spec/decision coverage:** scope=ingested-only (Task 6 hangs off `/creators/:id`, no person-search) ✓; minimal display (name + owned/add, one CV call — Tasks 4/6) ✓; owned/not-owned split alphabetical client-side (build_discovery sort + Task 6 filters) ✓; live no-cache (`state.cv.fetch_person_volume_credits` per request) ✓; show-all/no-filter ✓; no-role (CvVolumeCredit has none) ✓; acquire reuse (`addSeries` → `POST /api/series`) ✓.
- **Type consistency:** `CvVolumeCredit { cv_volume_id, name }` (Task 1) → `fetch_person_volume_credits` return (Task 2) → `build_discovery` input (Task 4). `DiscoveredVolume { cv_volume_id, name, series_id }` (Task 4) → TS `DiscoveredVolume` (Task 5) → Task 6 filters on `series_id`. `existing_cv_id_pairs -> Vec<(id, cv_id)>` (Task 3) → `build_discovery(_, &owned)` maps `(sid,cvid)->(cvid,sid)`. `cv_person_id_of -> Option<i64>` (Task 3) → handler `let Some(person_id) = ... else return []` (Task 4).
- **No migration, no cache table** — reuses `series.cv_id` + `creators.cv_person_id` + `POST /api/series`. CI (fmt + clippy -D warnings) enforced — every commit step runs `cargo fmt`.
