# Per-Volume Enrichment (Discovery) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enrich discovered (CV bibliography) volumes with publisher, year, and cover thumbnail, and add an additive publisher-blocklist filter on top of the existing static foreign-reprint markers — reusing the existing `cv_volume_cache` + fill-worker + `publisher_filters` infrastructure.

**Architecture:** The heavy lifting already exists. `cv_volume_cache` (a permanent per-CV-volume metadata cache) is drained by a continuous, rate-throttled fill worker (`longbox-cv-enrichment/worker.rs`), and `bulk_queue_pending` enqueues ids for it. This plan (1) adds a `cover_url` column to that cache + captures it in the worker, (2) makes the discovery route queue its volumes for enrichment, join the cache to surface publisher/year/cover, and additively apply the `publisher_filters` blocklist (mirroring `cv_search.rs`), and (3) renders covers + a "N filtered — show them" toggle in the shared `CreatorDiscovery.svelte`.

**Tech Stack:** Rust (longbox-db / sqlx SQLite offline, longbox-cv-enrichment, longbox-web / Axum 0.7), SvelteKit (Svelte 5 runes, pnpm). CI gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (all `SQLX_OFFLINE=true`), `pnpm build`.

**Key facts (verified 2026-07-01):**
- `cv_volume_cache`: `cv_volume_id PK, publisher, description, start_year, fetched_at (NULL=pending), first_seen_at`. **No `cover_url`.** `mark_fetched` writes publisher/description/start_year; the fill worker's `cache_fill_one` is its ONLY caller (plus a `#[allow(dead_code)]` test-only `find_by_id`).
- `bulk_queue_pending(pool, &[i64])` — `INSERT OR IGNORE` enqueues ids as pending (idempotent). Currently called only from `calendar.rs`.
- The fill worker (`worker.rs::cache_fill_one`) already calls `bg.fetch_volume(id)` → `CvVolumeDetail` which **already carries `cover_url: Option<String>`** — we just need to persist it.
- `cv_search.rs` is the reference for additive publisher filtering: `publisher_filter_repo::blocked_names_lower(db) -> Vec<String>` (lowercased block names), a `show_filtered: bool` query param, and a `{ results, filtered_count }` response.
- Discovery today: `discover_by_cv_person(state, cv_person_id)` → `fetch_person_volume_credits` + `existing_cv_id_pairs` + the pure `build_discovery` → `Vec<DiscoveredVolume { cv_volume_id, name, series_id }>`. Both the local-id route (`/creators/:id/discover`) and the cv-person route (`/creators/discover?cv_person_id=`) go through this helper. The static foreign filter (`FOREIGN_COLLECTION_MARKERS` + `is_foreign_reprint`) lives in `creators.rs` and stays.
- Cache read strategy: mirror `list_all_publishers` (fetch all cache rows once, build a HashMap) rather than a dynamic `IN` clause — keeps the compile-checked `query!` macro. `// ponytail: fetch-all + HashMap like the calendar path; if the cache grows to 100k+ rows, switch to a batched IN query.`

---

## File Structure

- `longbox-db/migrations/20260701140000_add_cv_volume_cache_cover.sql` — new: `ALTER TABLE cv_volume_cache ADD COLUMN cover_url TEXT;`
- `longbox-db/src/cv_volume_cache_repo.rs` — add `cover_url` to `CvVolumeCacheRow`; add `cover_url` param to `mark_fetched`; add `CvVolumeMeta` + `list_metadata_all`.
- `longbox-cv-enrichment/src/worker.rs` — `cache_fill_one` passes `volume.cover_url` to `mark_fetched`.
- `longbox-web/src/routes/creators.rs` — extend `DiscoveredVolume` (publisher/start_year/cover_url); refactor `build_discovery` to pure `(Vec<DiscoveredVolume>, u32)` with cache-meta + blocklist; `DiscoveryResponse` wrapper; both discover handlers queue + enrich + accept `show_filtered`.
- `longbox-frontend/src/lib/api/creators.ts` — extend `DiscoveredVolume`; `DiscoveryResponse` type; `show_filtered` params on both fetchers.
- `longbox-frontend/src/lib/components/CreatorDiscovery.svelte` — covers + publisher/year; "N filtered — show them" toggle.
- `longbox-frontend/src/routes/creators/[id]/+page.svelte` and `.../creators/+page.svelte` — adapt to the wrapper response + toggle callback.

---

## Task 1: Cache `cover_url` — migration + repo + worker

**Files:**
- Create: `longbox-db/migrations/20260701140000_add_cv_volume_cache_cover.sql`
- Modify: `longbox-db/src/cv_volume_cache_repo.rs`
- Modify: `longbox-cv-enrichment/src/worker.rs`

- [ ] **Step 1: Migration**

Create `longbox-db/migrations/20260701140000_add_cv_volume_cache_cover.sql`:

```sql
-- Cover URL for a cached CV volume (CV's medium cover image). NULL when
-- not yet fetched or when CV has no cover. Used by Discovery to show a
-- thumbnail per not-owned volume.
ALTER TABLE cv_volume_cache ADD COLUMN cover_url TEXT;
```

- [ ] **Step 2: Add `cover_url` to `CvVolumeCacheRow` + `find_by_id` select**

In `longbox-db/src/cv_volume_cache_repo.rs`, add the field to the struct (after `start_year`):

```rust
    pub start_year: Option<i64>,
    pub cover_url: Option<String>,
```

And add `cover_url` to the `find_by_id` `SELECT` (keep it compiling against the new column):

```rust
        r#"SELECT cv_volume_id AS "cv_volume_id!: i64",
                  publisher,
                  description,
                  start_year,
                  cover_url,
                  fetched_at AS "fetched_at: _",
                  first_seen_at AS "first_seen_at!: _"
           FROM cv_volume_cache
           WHERE cv_volume_id = ?"#,
```

- [ ] **Step 3: Extend `mark_fetched` with `cover_url`**

In `longbox-db/src/cv_volume_cache_repo.rs`, change `mark_fetched` to accept and write `cover_url`:

```rust
pub async fn mark_fetched<'e, E>(
    executor: E,
    cv_volume_id: i64,
    publisher: Option<&str>,
    description: Option<&str>,
    start_year: Option<i32>,
    cover_url: Option<&str>,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let start_year_i64 = start_year.map(i64::from);
    let result = sqlx::query!(
        r#"UPDATE cv_volume_cache
           SET publisher = ?, description = ?, start_year = ?, cover_url = ?,
               fetched_at = CURRENT_TIMESTAMP
           WHERE cv_volume_id = ?"#,
        publisher,
        description,
        start_year_i64,
        cover_url,
        cv_volume_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
```

- [ ] **Step 4: Add `CvVolumeMeta` + `list_metadata_all`**

In `longbox-db/src/cv_volume_cache_repo.rs`, add (after `list_all_publishers`):

```rust
/// Discovery-facing metadata for one cached volume. `publisher`/`start_year`/
/// `cover_url` are all `None` for a still-pending row.
#[derive(Debug, Clone, PartialEq)]
pub struct CvVolumeMeta {
    pub cv_volume_id: i64,
    pub publisher: Option<String>,
    pub start_year: Option<i64>,
    pub cover_url: Option<String>,
}

/// All cache rows projected to discovery metadata. Discovery reads this once
/// per request and builds a `HashMap<cv_volume_id, CvVolumeMeta>` for in-memory
/// joins — mirrors `list_all_publishers` (the calendar path).
// ponytail: fetch-all + HashMap like the calendar path; if the cache grows to
// 100k+ rows, switch to a batched IN query keyed on the discovered ids.
pub async fn list_metadata_all<'e, E>(executor: E) -> Result<Vec<CvVolumeMeta>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query_as!(
        CvVolumeMeta,
        r#"SELECT cv_volume_id AS "cv_volume_id!: i64",
                  publisher,
                  start_year,
                  cover_url
           FROM cv_volume_cache"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}
```

- [ ] **Step 5: Capture cover in the fill worker**

In `longbox-cv-enrichment/src/worker.rs`, `cache_fill_one`, add the new arg to the `mark_fetched` call:

```rust
    match cv_volume_cache_repo::mark_fetched(
        db,
        cv_volume_id,
        volume.publisher.as_deref(),
        volume.description.as_deref(),
        volume.start_year,
        volume.cover_url.as_deref(),
    )
    .await
```

(`volume` is a `CvVolumeDetail`, which has `cover_url: Option<String>` — confirm by reading the struct. No other `mark_fetched` call sites exist.)

- [ ] **Step 6: Regenerate sqlx offline metadata**

The `mark_fetched` UPDATE changed and `list_metadata_all` is new, so `.sqlx/` must be regenerated:

```bash
export DATABASE_URL="sqlite:///tmp/lb-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
cargo sqlx prepare --workspace -- --all-targets
rm -f /tmp/lb-prepare.db
unset DATABASE_URL
```
Then `git status .sqlx` — expect additions for the new/changed queries; the old `mark_fetched` query file will be replaced (its hash changes). Verify no UNRELATED `.sqlx` files vanished (the suite has test-only queries — if any disappear, STOP and report).

- [ ] **Step 7: Verify**

```bash
SQLX_OFFLINE=true cargo clippy -p longbox-db -p longbox-cv-enrichment --all-targets -- -D warnings
SQLX_OFFLINE=true cargo test -p longbox-db
```
Both clean. (If a `cv_volume_cache_repo` test calls `mark_fetched`, update it to pass the new `cover_url` arg, e.g. `Some("https://cv/cover.jpg")`, and assert it round-trips via `find_by_id`.)

- [ ] **Step 8: Commit**

```bash
git add longbox-db/migrations/20260701140000_add_cv_volume_cache_cover.sql longbox-db/src/cv_volume_cache_repo.rs longbox-cv-enrichment/src/worker.rs .sqlx
git commit -m "feat(db): cache cv volume cover_url + list_metadata_all"
```

---

## Task 2: Discovery route — queue + enrich + additive publisher filter

**Files:**
- Modify: `longbox-web/src/routes/creators.rs`

- [ ] **Step 1: Extend `DiscoveredVolume` + add the response wrapper**

In `longbox-web/src/routes/creators.rs`, replace the `DiscoveredVolume` struct with the enriched version and add a wrapper. Add imports `use longbox_db::cv_volume_cache_repo::{self, CvVolumeMeta};` and `use longbox_db::publisher_filter_repo;` (match the existing `use longbox_db::{...}` grouping style):

```rust
/// One series in a creator's CV bibliography. `series_id` is `Some(local id)`
/// when the volume is already in the library. `publisher`/`start_year`/
/// `cover_url` come from `cv_volume_cache` — `None` until the fill worker
/// enriches the id (eventually-consistent).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct DiscoveredVolume {
    cv_volume_id: i64,
    name: String,
    series_id: Option<i64>,
    publisher: Option<String>,
    start_year: Option<i64>,
    cover_url: Option<String>,
}

/// Discovery response: the volumes plus how many NOT-owned volumes the
/// publisher blocklist removed (0 when `show_filtered=true`). Mirrors
/// `cv_search.rs`'s `{ results, filtered_count }` shape.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct DiscoveryResponse {
    results: Vec<DiscoveredVolume>,
    filtered_count: u32,
}
```

- [ ] **Step 2: Write the failing enrichment + filter test**

In `creators.rs`'s `#[cfg(test)] mod discover_tests`, add (uses the new `build_discovery` signature):

```rust
#[test]
fn build_discovery_enriches_and_publisher_filters() {
    use std::collections::HashMap;
    let credits = vec![
        CvVolumeCredit { cv_volume_id: 10, name: "Saga".into() },       // owned
        CvVolumeCredit { cv_volume_id: 20, name: "Panini Reprint".into() }, // static-foreign -> dropped silently
        CvVolumeCredit { cv_volume_id: 30, name: "Nailbiter".into() },  // not-owned, publisher Image -> kept
        CvVolumeCredit { cv_volume_id: 40, name: "Blocked Book".into() }, // not-owned, publisher "Panini" -> blocklist drop
    ];
    let owned = vec![(3_i64, 10_i64)];
    let mut meta = HashMap::new();
    meta.insert(30, CvVolumeMeta { cv_volume_id: 30, publisher: Some("Image".into()), start_year: Some(2014), cover_url: Some("http://c/30.jpg".into()) });
    meta.insert(40, CvVolumeMeta { cv_volume_id: 40, publisher: Some("Panini".into()), start_year: Some(2010), cover_url: None });
    let blocked = vec!["panini".to_string()];
    let (results, filtered) = build_discovery(credits, &owned, &meta, &blocked);
    // "Panini Reprint" (static) silently gone; "Blocked Book" (publisher) filtered+counted.
    assert_eq!(filtered, 1);
    let names: Vec<&str> = results.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, vec!["Nailbiter", "Saga"]); // case-insensitive sort
    let saga = results.iter().find(|v| v.cv_volume_id == 10).unwrap();
    assert_eq!(saga.series_id, Some(3));
    let nail = results.iter().find(|v| v.cv_volume_id == 30).unwrap();
    assert_eq!(nail.publisher.as_deref(), Some("Image"));
    assert_eq!(nail.start_year, Some(2014));
    assert_eq!(nail.cover_url.as_deref(), Some("http://c/30.jpg"));
}
```

Run: `SQLX_OFFLINE=true cargo test -p longbox-web build_discovery_enriches_and_publisher_filters` → FAIL (signature mismatch / fields missing).

- [ ] **Step 3: Refactor `build_discovery` to enrich + additively filter**

Replace the existing `build_discovery` in `creators.rs` with:

```rust
/// Pure join+filter+sort. Maps each CV volume credit to owned/not-owned
/// against `owned_pairs`, enriches from `cache_meta`, drops static foreign
/// reprints (silently) and NOT-owned volumes whose cached publisher is in
/// `blocked_publishers` (counted). Owned volumes are never publisher-filtered.
/// Returns (results sorted by name, count removed by the publisher blocklist).
fn build_discovery(
    credits: Vec<CvVolumeCredit>,
    owned_pairs: &[(i64, i64)],
    cache_meta: &HashMap<i64, CvVolumeMeta>,
    blocked_publishers: &[String],
) -> (Vec<DiscoveredVolume>, u32) {
    let owned: HashMap<i64, i64> = owned_pairs.iter().map(|(sid, cvid)| (*cvid, *sid)).collect();
    let mut filtered_count = 0u32;
    let mut out: Vec<DiscoveredVolume> = credits
        .into_iter()
        .filter(|c| !is_foreign_reprint(&c.name)) // static markers: silent drop
        .filter_map(|c| {
            let series_id = owned.get(&c.cv_volume_id).copied();
            let meta = cache_meta.get(&c.cv_volume_id);
            let publisher = meta.and_then(|m| m.publisher.clone());
            // Publisher blocklist applies only to NOT-owned volumes.
            if series_id.is_none() {
                if let Some(p) = publisher.as_deref() {
                    if blocked_publishers.iter().any(|b| b == &p.to_lowercase()) {
                        filtered_count += 1;
                        return None;
                    }
                }
            }
            Some(DiscoveredVolume {
                cv_volume_id: c.cv_volume_id,
                name: c.name,
                series_id,
                publisher,
                start_year: meta.and_then(|m| m.start_year),
                cover_url: meta.and_then(|m| m.cover_url.clone()),
            })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    (out, filtered_count)
}
```

Run the test again → PASS.

- [ ] **Step 4: Update the shared helper to queue + read cache + filter**

Replace `discover_by_cv_person` in `creators.rs` with:

```rust
/// Shared: a CV person's series bibliography, owned/not-owned flagged and
/// enriched from cv_volume_cache. Queues the NOT-owned volume ids for the
/// enrichment worker (fire-and-forget), then joins the cache + applies the
/// publisher blocklist (unless `show_filtered`). One live CV call.
async fn discover_by_cv_person(
    state: &AppState,
    cv_person_id: i64,
    show_filtered: bool,
) -> Result<DiscoveryResponse, ApiError> {
    let credits = state.cv.fetch_person_volume_credits(cv_person_id).await?;
    let owned = series_repo::existing_cv_id_pairs(&state.db).await?;
    let owned_ids: std::collections::HashSet<i64> = owned.iter().map(|(_, cvid)| *cvid).collect();

    // Queue not-owned volumes for background enrichment (idempotent INSERT OR IGNORE).
    let to_queue: Vec<i64> = credits
        .iter()
        .map(|c| c.cv_volume_id)
        .filter(|id| !owned_ids.contains(id))
        .collect();
    if let Err(e) = cv_volume_cache_repo::bulk_queue_pending(&state.db, &to_queue).await {
        // Non-fatal: enrichment is a nice-to-have; log and serve bare metadata.
        tracing::warn!(target: "longbox_web", error = %e, "discovery enqueue failed");
    }

    let cache_meta: HashMap<i64, CvVolumeMeta> = cv_volume_cache_repo::list_metadata_all(&state.db)
        .await?
        .into_iter()
        .map(|m| (m.cv_volume_id, m))
        .collect();
    let blocked = if show_filtered {
        Vec::new()
    } else {
        publisher_filter_repo::blocked_names_lower(&state.db).await?
    };
    let (results, filtered_count) = build_discovery(credits, &owned, &cache_meta, &blocked);
    Ok(DiscoveryResponse { results, filtered_count })
}
```

- [ ] **Step 5: Update both discover handlers for the wrapper + `show_filtered`**

Replace the two handlers. Add a shared query-params struct. The local-id route keeps `Path(id)`; both take an optional `show_filtered`:

```rust
#[derive(Debug, Deserialize)]
struct DiscoverParams {
    #[serde(default)]
    show_filtered: bool,
}

/// Discover by LOCAL creator id. Empty when the creator has no cv_person_id.
async fn discover(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<DiscoverParams>,
) -> Result<Json<DiscoveryResponse>, ApiError> {
    let Some(person_id) = creator_repo::cv_person_id_of(&state.db, id).await? else {
        return Ok(Json(DiscoveryResponse { results: Vec::new(), filtered_count: 0 }));
    };
    Ok(Json(discover_by_cv_person(&state, person_id, params.show_filtered).await?))
}

#[derive(Debug, Deserialize)]
struct DiscoverCvParams {
    cv_person_id: i64,
    #[serde(default)]
    show_filtered: bool,
}

async fn discover_cv_handler(
    State(state): State<AppState>,
    Query(params): Query<DiscoverCvParams>,
) -> Result<Json<DiscoveryResponse>, ApiError> {
    Ok(Json(discover_by_cv_person(&state, params.cv_person_id, params.show_filtered).await?))
}
```

Ensure `HashMap` is imported (already is, per Task from the prior feature). Router lines are unchanged.

- [ ] **Step 6: Verify**

```bash
SQLX_OFFLINE=true cargo clippy -p longbox-web --all-targets -- -D warnings
SQLX_OFFLINE=true cargo test -p longbox-web
SQLX_OFFLINE=true cargo check --workspace
```
All clean. Update any existing `build_discovery` / discovery tests to the new signature/return (the older `build_discovery_maps_owned_and_sorts_case_insensitive` and `build_discovery_drops_foreign_reprints` tests will need the two extra args `&HashMap::new(), &[]` and to destructure `(out, _)`).

- [ ] **Step 7: Commit**

```bash
git add longbox-web/src/routes/creators.rs
git commit -m "feat(web): discovery enrichment (publisher/year/cover) + additive publisher filter"
```

---

## Task 3: Frontend — covers + publisher/year + filter toggle

**Files:**
- Modify: `longbox-frontend/src/lib/api/creators.ts`
- Modify: `longbox-frontend/src/lib/components/CreatorDiscovery.svelte`
- Modify: `longbox-frontend/src/routes/creators/[id]/+page.svelte`
- Modify: `longbox-frontend/src/routes/creators/+page.svelte`

- [ ] **Step 1: API types + fetcher signatures**

In `longbox-frontend/src/lib/api/creators.ts`, extend `DiscoveredVolume` and add a response type; change both discovery fetchers to return `DiscoveryResponse` and accept `showFiltered`:

```typescript
export interface DiscoveredVolume {
  cv_volume_id: number;
  name: string;
  series_id: number | null; // non-null => already in the library
  publisher: string | null;
  start_year: number | null;
  cover_url: string | null;
}

export interface DiscoveryResponse {
  results: DiscoveredVolume[];
  filtered_count: number;
}

export function getCreatorDiscovery(id: number, showFiltered = false): Promise<DiscoveryResponse> {
  return apiFetch(`/creators/${id}/discover${showFiltered ? '?show_filtered=true' : ''}`);
}

export function discoverByCvPerson(cvPersonId: number, showFiltered = false): Promise<DiscoveryResponse> {
  const sf = showFiltered ? '&show_filtered=true' : '';
  return apiFetch(`/creators/discover?cv_person_id=${cvPersonId}${sf}`);
}
```

- [ ] **Step 2: Shared component — covers, metadata, filter toggle**

Rewrite `longbox-frontend/src/lib/components/CreatorDiscovery.svelte`. It takes the loaded `volumes` + `filteredCount` + an `onShowFiltered` callback (the parent re-fetches because the fetch URL differs per caller):

```svelte
<script lang="ts">
  import { type DiscoveredVolume } from '$lib/api/creators';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';

  let {
    volumes,
    filteredCount = 0,
    onShowFiltered,
  }: {
    volumes: DiscoveredVolume[];
    filteredCount?: number;
    onShowFiltered?: () => void;
  } = $props();

  let addingId = $state<number | null>(null);
  let addedIds = $state<Set<number>>(new Set());

  const inLibrary = $derived(volumes.filter((d) => d.series_id !== null));
  const notInLibrary = $derived(volumes.filter((d) => d.series_id === null));

  async function acquire(cvVolumeId: number) {
    addingId = cvVolumeId;
    try {
      await addSeries(cvVolumeId);
      addedIds = new Set(addedIds).add(cvVolumeId);
    } finally {
      addingId = null;
    }
  }

  function meta(v: DiscoveredVolume): string {
    return [v.start_year, v.publisher].filter(Boolean).join(' · ');
  }
</script>

{#if volumes.length === 0}
  <p class="text-sm text-slate-500">No series found for this creator.</p>
{:else}
  <h3 class="mb-2 text-lg font-semibold">Not in your library ({notInLibrary.length})</h3>
  <ul class="mb-2 space-y-2">
    {#each notInLibrary as v (v.cv_volume_id)}
      <li class="flex items-center justify-between gap-3">
        <div class="flex min-w-0 items-center gap-2">
          {#if v.cover_url}
            <img src={v.cover_url} alt={v.name} class="h-12 w-8 shrink-0 rounded object-cover" />
          {:else}
            <div class="h-12 w-8 shrink-0 rounded bg-slate-100"></div>
          {/if}
          <div class="min-w-0">
            <div class="truncate">{v.name}</div>
            {#if meta(v)}<div class="text-xs text-slate-400">{meta(v)}</div>{/if}
          </div>
        </div>
        {#if addedIds.has(v.cv_volume_id)}
          <span class="shrink-0 text-sm text-green-600">✓ Added</span>
        {:else}
          <Button
            variant="secondary"
            size="sm"
            loading={addingId === v.cv_volume_id}
            onclick={() => acquire(v.cv_volume_id)}
          >Add to Library</Button>
        {/if}
      </li>
    {/each}
  </ul>

  {#if filteredCount > 0 && onShowFiltered}
    <button class="mb-4 text-sm text-blue-600 hover:underline" onclick={onShowFiltered}>
      {filteredCount} foreign-reprint {filteredCount === 1 ? 'edition' : 'editions'} filtered — show them
    </button>
  {/if}

  <h3 class="mb-2 text-lg font-semibold">In your library ({inLibrary.length})</h3>
  <ul class="space-y-1">
    {#each inLibrary as v (v.cv_volume_id)}
      <li><a href={`/series/${v.series_id}`} class="hover:underline">{v.name}</a></li>
    {/each}
  </ul>
{/if}
```

- [ ] **Step 3: Adapt the creator-detail page**

In `longbox-frontend/src/routes/creators/[id]/+page.svelte`: `discovery` now holds a `DiscoveryResponse` (or null). Add a `showDiscoveryFiltered` state and a reload that passes it. Update the loader + the component usage:

```svelte
  import { getCreatorDiscovery, type DiscoveryResponse } from '$lib/api/creators';
  import CreatorDiscovery from '$lib/components/CreatorDiscovery.svelte';
  // ...
  let discovery = $state<DiscoveryResponse | null>(null);
  let discovering = $state(false);
  let discoverError = $state<string | null>(null);
  let showFiltered = $state(false);

  async function loadDiscovery() {
    discovering = true;
    discoverError = null;
    try {
      discovery = await getCreatorDiscovery(data.creator.id, showFiltered);
    } catch (e) {
      discoverError = e instanceof Error ? e.message : 'Failed to load bibliography';
    } finally {
      discovering = false;
    }
  }

  function revealFiltered() {
    showFiltered = true;
    loadDiscovery();
  }
```

And where it renders:

```svelte
{#if discovery !== null}
  <CreatorDiscovery
    volumes={discovery.results}
    filteredCount={discovery.filtered_count}
    onShowFiltered={revealFiltered}
  />
{/if}
```

- [ ] **Step 4: Adapt the creators index (ComicVine fallback)**

In `longbox-frontend/src/routes/creators/+page.svelte`: the expand loads a `DiscoveryResponse`. Change `expandedVolumes` to hold the response, and add a reveal that re-fetches with `showFiltered=true` for the currently-expanded person:

```svelte
  import {
    searchCreators, searchCvCreators, discoverByCvPerson,
    type CreatorSearchRow, type CvCreatorCandidate, type DiscoveryResponse,
  } from '$lib/api/creators';
  // ...
  let expanded = $state<number | null>(null);
  let expandedData = $state<DiscoveryResponse | null>(null);
  let expandLoading = $state(false);

  async function toggleDiscover(cvPersonId: number) {
    if (expanded === cvPersonId) { expanded = null; expandedData = null; return; }
    expanded = cvPersonId;
    expandedData = null;
    expandLoading = true;
    try {
      expandedData = await discoverByCvPerson(cvPersonId);
    } finally {
      expandLoading = false;
    }
  }

  async function revealFiltered(cvPersonId: number) {
    expandLoading = true;
    try {
      expandedData = await discoverByCvPerson(cvPersonId, true);
    } finally {
      expandLoading = false;
    }
  }
```

And the render block for the expanded person:

```svelte
{#if expanded === p.cv_person_id}
  {#if expandLoading}
    <p class="mt-2 text-sm text-slate-500">Loading bibliography…</p>
  {:else if expandedData !== null}
    <CreatorDiscovery
      volumes={expandedData.results}
      filteredCount={expandedData.filtered_count}
      onShowFiltered={() => revealFiltered(p.cv_person_id)}
    />
  {/if}
{/if}
```

Also reset `expandedData` (not `expandedVolumes`) wherever `onInput` clears expansion state.

- [ ] **Step 5: Build (CI gate)**

```bash
cd longbox-frontend && pnpm build
```
Must succeed. Fix any TS errors properly (the response shape changed from array → object — the compiler will point at every stale `.filter`/`.length` on the old array type).

- [ ] **Step 6: Restore the frontend-dist placeholder**

```bash
cd /Users/jeremy/Projects/longbox
git checkout HEAD -- longbox-web/frontend-dist/index.html 2>/dev/null
git status --short longbox-web/frontend-dist
```
No bundle artifacts staged.

- [ ] **Step 7: Commit**

```bash
git add longbox-frontend/src/lib/api/creators.ts longbox-frontend/src/lib/components/CreatorDiscovery.svelte "longbox-frontend/src/routes/creators/[id]/+page.svelte" longbox-frontend/src/routes/creators/+page.svelte
git commit -m "feat(frontend): discovery covers + publisher/year + foreign-filter reveal toggle"
```

---

## Final Verification (after all tasks)

```bash
SQLX_OFFLINE=true cargo fmt --all -- --check
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
SQLX_OFFLINE=true cargo test --workspace
(cd longbox-frontend && pnpm build)
```
Expected: fmt clean, clippy 0, all tests pass, frontend builds.

**Manual smoke (after deploy):** Discover a creator with foreign reprints (e.g. a Marvel-heavy creator). First view: names + "(0)"-ish metadata, publisher/year/covers fill in on subsequent views as the worker drains the queue (watch `cv_volume_cache.filled` logs and `SELECT COUNT(*) FROM cv_volume_cache WHERE fetched_at IS NOT NULL`). Confirm publisher-blocked not-owned volumes drop out and the "N foreign-reprint editions filtered — show them" toggle reveals them.

---

## Self-Review

**Spec coverage:**
- Metadata (publisher+year) on discovered volumes → Task 1 (cache read) + Task 2 (`build_discovery` enrich) + Task 3 (render). ✅
- Covers → Task 1 (cover_url column + worker capture + `list_metadata_all`) + Task 2 (surface) + Task 3 (thumbnail). ✅
- Additive publisher blocklist (keep static markers + layer publisher_filters, with toggle) → Task 2 (`build_discovery` keeps `is_foreign_reprint` AND applies `blocked_publishers`; `filtered_count`; `show_filtered`) + Task 3 (reveal toggle). ✅
- Queue discovered volumes for enrichment → Task 2 (`bulk_queue_pending` on not-owned ids). ✅
- Eventually-consistent (no live per-volume fetch) → yes; the fill worker drains async. ✅

**Placeholder scan:** No TBD/TODO; every code step is complete. sqlx-prepare + test-update steps are concrete instructions, not placeholders.

**Type consistency:** `CvVolumeMeta` (db) fields (`cv_volume_id, publisher, start_year, cover_url`) match the join in `build_discovery`. `DiscoveredVolume` gains the same three fields Rust↔TS (`publisher: Option<String>`↔`string|null`, `start_year: Option<i64>`↔`number|null`, `cover_url`↔`string|null`). `DiscoveryResponse { results, filtered_count }` matches TS. `mark_fetched`'s new `cover_url: Option<&str>` matches the worker's `volume.cover_url.as_deref()`. Both discover routes return `Json<DiscoveryResponse>`; both TS fetchers return `DiscoveryResponse`. `show_filtered` param wired end-to-end.
