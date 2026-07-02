# Arbitrary Creator Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user search ComicVine for *any* creator (including ones they own nothing by) from the `/creators` page, see that creator's series bibliography with owned/not-owned flags, and acquire volumes — reusing the existing Discovery join and acquire flow.

**Architecture:** Three thin layers, no schema change, no background job, fully live/interactive. (1) A new CV client method `search_persons` maps `search/?resources=person` to a projected `CvPersonSearchResult`. (2) Two new web routes: `GET /api/creators/cv-search?q=` (person candidates, each flagged if already a local creator) and `GET /api/creators/discover?cv_person_id=` (bibliography for a raw CV person id, reusing the existing pure `build_discovery`). (3) Frontend: an auto-fallback "Not in your library · ComicVine" section on `/creators`, plus a shared `CreatorDiscovery.svelte` component used by both the fallback and the existing creator-detail page.

**Tech Stack:** Rust (longbox-comicvine, longbox-db, longbox-web / Axum 0.7, sqlx offline), SvelteKit (Svelte 5 runes, pnpm). CI gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (all `SQLX_OFFLINE=true`), `pnpm build`.

**Key facts (verified live 2026-07-01):**
- `GET search/?resources=person&query=<q>&field_list=id,name,deck,image,country&limit=<n>` returns ranked people. `deck` is a one-line bio (great disambiguator: Fiona vs Greg vs Val Staples), `country` present, `image` is the standard CV image object (`medium_url`). Person search results do NOT carry a usable issue count (`count_of_isssue_appearances` is null), so we don't show one.
- The existing `discover` handler (`longbox-web/src/routes/creators.rs`) already resolves a *local* creator id → `cv_person_id` → `fetch_person_volume_credits` → `build_discovery`. `build_discovery` is a pure function already unit-tested (owned/not-owned join, foreign-reprint filter, case-insensitive sort). We reuse it verbatim.
- `creators` schema already has `cv_person_id INTEGER UNIQUE`. No migration needed.

---

## File Structure

- `longbox-comicvine/src/models.rs` — add `CvPersonSearchItem` raw DTO.
- `longbox-comicvine/src/projection.rs` — add public `CvPersonSearchResult` + `project_person_search_item` + unit test.
- `longbox-comicvine/src/client.rs` — add `search_persons` method.
- `longbox-comicvine/src/lib.rs` — export `CvPersonSearchResult`.
- `longbox-db/src/creator_repo.rs` — add `cv_person_id_map` (cv_person_id → local creator id pairs).
- `longbox-web/src/routes/creators.rs` — add `CvCreatorCandidate`, pure `flag_candidates` + test, shared `discover_by_cv_person`, refactor `discover`, add `cv_search_handler` + `discover_cv_handler`, register routes.
- `longbox-frontend/src/lib/api/creators.ts` — add candidate/discovery-by-person types + fetchers.
- `longbox-frontend/src/lib/components/CreatorDiscovery.svelte` — new shared component (owned/not-owned split + acquire).
- `longbox-frontend/src/routes/creators/[id]/+page.svelte` — use the shared component.
- `longbox-frontend/src/routes/creators/+page.svelte` — add the ComicVine fallback section.

---

## Task 1: CV client `search_persons`

**Files:**
- Modify: `longbox-comicvine/src/models.rs`
- Modify: `longbox-comicvine/src/projection.rs`
- Modify: `longbox-comicvine/src/client.rs`
- Modify: `longbox-comicvine/src/lib.rs`

- [ ] **Step 1: Add the raw DTO**

In `longbox-comicvine/src/models.rs`, after `CvVolumeSearchItem` (ends ~line 60), add:

```rust
/// One entry from `search/?resources=person`. `deck` is CV's one-line bio;
/// `image` / `country` are optional. Person search results carry no usable
/// issue count, so we don't model one.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CvPersonSearchItem {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub deck: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub image: Option<CvImage>,
}
```

- [ ] **Step 2: Write the failing projection test**

In `longbox-comicvine/src/projection.rs`, inside the existing `#[cfg(test)] mod tests` block (add a new test fn; if the test module uses `use crate::models::{...}`, extend it to include `CvPersonSearchItem`):

```rust
#[test]
fn project_person_search_item_maps_fields() {
    let item = CvPersonSearchItem {
        id: 52884,
        name: "Fiona Staples".into(),
        deck: Some("Canadian comic book artist.".into()),
        country: Some("Canada".into()),
        image: Some(CvImage {
            medium_url: Some("https://cv/img/med.jpg".into()),
        }),
    };
    let out = project_person_search_item(item);
    assert_eq!(
        out,
        CvPersonSearchResult {
            cv_person_id: 52884,
            name: "Fiona Staples".into(),
            description: Some("Canadian comic book artist.".into()),
            country: Some("Canada".into()),
            image_url: Some("https://cv/img/med.jpg".into()),
        }
    );
}
```

Make sure the test module imports resolve: at the top of the `#[cfg(test)] mod tests` block add `CvPersonSearchItem` and `CvImage` to the `use crate::models::{...}` line if not already present.

- [ ] **Step 3: Run the test to verify it fails**

Run: `SQLX_OFFLINE=true cargo test -p longbox-comicvine project_person_search_item_maps_fields`
Expected: FAIL — `CvPersonSearchResult` / `project_person_search_item` not found (compile error).

- [ ] **Step 4: Add the projected type + projector**

In `longbox-comicvine/src/projection.rs`, after `CvVolumeCredit` / `project_person_volume_credits` (ends ~line 118), add:

```rust
/// A ComicVine person surfaced by `search_persons`. `description` is CV's
/// one-line `deck` (disambiguator); `image_url` is the medium cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CvPersonSearchResult {
    pub cv_person_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub country: Option<String>,
    pub image_url: Option<String>,
}

pub(crate) fn project_person_search_item(item: CvPersonSearchItem) -> CvPersonSearchResult {
    CvPersonSearchResult {
        cv_person_id: item.id,
        name: item.name,
        description: item.deck,
        country: item.country,
        image_url: extract_cover(item.image),
    }
}
```

Ensure the file's top-level `use crate::models::{...}` (around line 1-10) includes `CvPersonSearchItem`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `SQLX_OFFLINE=true cargo test -p longbox-comicvine project_person_search_item_maps_fields`
Expected: PASS.

- [ ] **Step 6: Add the client method**

In `longbox-comicvine/src/client.rs`, after `search_volumes` (ends ~line 198), add — mirror `search_volumes` exactly, swapping the resource and projector. Note `field_list` requests the fields the projection needs:

```rust
#[instrument(target = "longbox_comicvine", skip(self), fields(query = %query))]
pub async fn search_persons(&self, query: &str) -> Result<Vec<CvPersonSearchResult>, CvError> {
    let limit = PAGE_LIMIT.to_string();
    let url = self.build_url(
        "search/",
        &[
            ("resources", "person"),
            ("query", query),
            ("field_list", "id,name,deck,image,country"),
            ("limit", limit.as_str()),
        ],
    )?;
    let body = self.execute_with_retry(url).await?;
    let envelope = parse_envelope::<Vec<CvPersonSearchItem>>(&body)?;
    let results = unwrap_envelope_results(envelope, &body)?;
    Ok(results.into_iter().map(project_person_search_item).collect())
}
```

Add `CvPersonSearchItem` to the `use crate::models::{...}` import and `project_person_search_item` + `CvPersonSearchResult` to the `use crate::projection::{...}` import at the top of `client.rs` (match how `CvVolumeSearchItem` / `project_search_item` are already imported).

- [ ] **Step 7: Export the public type**

In `longbox-comicvine/src/lib.rs`, add `CvPersonSearchResult` to the `pub use` re-export list that already exports `SeriesSearchResult`, `CvVolumeCredit`, etc. (around line 21-22).

- [ ] **Step 8: Verify the crate builds + lints**

Run: `SQLX_OFFLINE=true cargo clippy -p longbox-comicvine --all-targets -- -D warnings && SQLX_OFFLINE=true cargo test -p longbox-comicvine`
Expected: clean, all tests pass.

- [ ] **Step 9: Commit**

```bash
git add longbox-comicvine/src/models.rs longbox-comicvine/src/projection.rs longbox-comicvine/src/client.rs longbox-comicvine/src/lib.rs
git commit -m "feat(comicvine): search_persons + CvPersonSearchResult"
```

---

## Task 2: Web routes — CV person search + discover-by-cv_person_id

**Files:**
- Modify: `longbox-db/src/creator_repo.rs`
- Modify: `longbox-web/src/routes/creators.rs`

- [ ] **Step 1: Add the repo work-list query**

In `longbox-db/src/creator_repo.rs`, after `cv_person_id_of` (ends ~line 268), add — mirror `series_repo::existing_cv_id_pairs`:

```rust
/// All `(cv_person_id, local creator id)` pairs for creators that carry a
/// CV person id. Small table (~2k rows); used to flag CV person-search hits
/// that are already in the library.
pub async fn cv_person_id_map<'e, E>(executor: E) -> Result<Vec<(i64, i64)>>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query!(
        r#"SELECT cv_person_id AS "cv_person_id!: i64", id AS "id!: i64"
           FROM creators WHERE cv_person_id IS NOT NULL"#
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| (r.cv_person_id, r.id)).collect())
}
```

- [ ] **Step 2: Regenerate sqlx offline metadata (new query)**

Run: `SQLX_OFFLINE=false DATABASE_URL="sqlite::memory:" cargo sqlx prepare --workspace -- --all-targets`

If that fails because the in-memory DB has no schema, use the project's standard prepare path against a migrated temp DB:
```bash
export DATABASE_URL="sqlite:///tmp/lb-prepare.db?mode=rwc"
cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
cargo sqlx prepare --workspace -- --all-targets
rm -f /tmp/lb-prepare.db
unset DATABASE_URL
```
Expected: `.sqlx/` gains a query file for the new `cv_person_id_map` statement; existing `.sqlx` files unchanged. Verify with `git status .sqlx` (only additions).

- [ ] **Step 3: Write the failing pure-flagging test**

In `longbox-web/src/routes/creators.rs`, add to the `#[cfg(test)] mod discover_tests` block (or a new test module `cv_search_tests`):

```rust
#[test]
fn flag_candidates_marks_in_library_and_preserves_order() {
    use longbox_comicvine::CvPersonSearchResult;
    let persons = vec![
        CvPersonSearchResult {
            cv_person_id: 52884,
            name: "Fiona Staples".into(),
            description: Some("Saga artist".into()),
            country: Some("Canada".into()),
            image_url: None,
        },
        CvPersonSearchResult {
            cv_person_id: 999,
            name: "Nobody Owned".into(),
            description: None,
            country: None,
            image_url: None,
        },
    ];
    // Local creator 7 owns cv person 52884; 999 is not local.
    let map = vec![(52884_i64, 7_i64)];
    let out = flag_candidates(persons, &map);
    assert_eq!(out[0].in_library_creator_id, Some(7));
    assert_eq!(out[0].cv_person_id, 52884);
    assert_eq!(out[1].in_library_creator_id, None);
    assert_eq!(out.len(), 2); // order preserved (CV rank)
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `SQLX_OFFLINE=true cargo test -p longbox-web flag_candidates_marks_in_library`
Expected: FAIL — `CvCreatorCandidate` / `flag_candidates` not found.

- [ ] **Step 5: Add the candidate type + pure flagger**

In `longbox-web/src/routes/creators.rs`, add near the top-level types (after `DiscoveredVolume`, ~line 91). Add `use longbox_comicvine::CvPersonSearchResult;` to the existing `use longbox_comicvine::...` line and `use std::collections::HashMap;` is already imported:

```rust
/// A ComicVine person-search hit, flagged with the local creator id when the
/// person is already in the library (so the UI can badge + link them instead
/// of offering discovery). Preserves CV's ranking order.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct CvCreatorCandidate {
    cv_person_id: i64,
    name: String,
    description: Option<String>,
    country: Option<String>,
    image_url: Option<String>,
    in_library_creator_id: Option<i64>,
}

/// Pure join: flag each CV person hit with its local creator id, if any.
/// `cv_to_local` is `(cv_person_id, creator_id)` pairs from
/// `creator_repo::cv_person_id_map`.
fn flag_candidates(
    persons: Vec<CvPersonSearchResult>,
    cv_to_local: &[(i64, i64)],
) -> Vec<CvCreatorCandidate> {
    let map: HashMap<i64, i64> = cv_to_local.iter().copied().collect();
    persons
        .into_iter()
        .map(|p| CvCreatorCandidate {
            in_library_creator_id: map.get(&p.cv_person_id).copied(),
            cv_person_id: p.cv_person_id,
            name: p.name,
            description: p.description,
            country: p.country,
            image_url: p.image_url,
        })
        .collect()
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `SQLX_OFFLINE=true cargo test -p longbox-web flag_candidates_marks_in_library`
Expected: PASS.

- [ ] **Step 7: Extract the shared discover helper + refactor existing `discover`**

In `longbox-web/src/routes/creators.rs`, replace the existing `discover` handler (currently ~line 149-159) with a shared helper plus the thin handler:

```rust
/// Shared: a CV person's series bibliography, owned/not-owned flagged.
/// One live CV call (person volume_credits) + the pure `build_discovery` join.
async fn discover_by_cv_person(
    state: &AppState,
    cv_person_id: i64,
) -> Result<Vec<DiscoveredVolume>, ApiError> {
    let credits = state.cv.fetch_person_volume_credits(cv_person_id).await?;
    let owned = series_repo::existing_cv_id_pairs(&state.db).await?;
    Ok(build_discovery(credits, &owned))
}

/// Discover by LOCAL creator id (existing route). Empty when the creator has
/// no known cv_person_id.
async fn discover(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<DiscoveredVolume>>, ApiError> {
    let Some(person_id) = creator_repo::cv_person_id_of(&state.db, id).await? else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(discover_by_cv_person(&state, person_id).await?))
}
```

- [ ] **Step 8: Add the CV-search + discover-by-cv_person handlers + params**

In `longbox-web/src/routes/creators.rs`, add these handlers (place near `search_handler`). `SearchParams` (with `q: String`) already exists — reuse it for the CV search handler:

```rust
async fn cv_search_handler(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<CvCreatorCandidate>>, ApiError> {
    let q = params.q.trim();
    if q.chars().count() < 2 {
        return Err(ApiError::BadRequest {
            message: "query parameter `q` must be at least 2 characters".into(),
        });
    }
    let persons = state.cv.search_persons(q).await?;
    let map = creator_repo::cv_person_id_map(&state.db).await?;
    Ok(Json(flag_candidates(persons, &map)))
}

#[derive(Debug, Deserialize)]
struct DiscoverCvParams {
    cv_person_id: i64,
}

async fn discover_cv_handler(
    State(state): State<AppState>,
    Query(params): Query<DiscoverCvParams>,
) -> Result<Json<Vec<DiscoveredVolume>>, ApiError> {
    Ok(Json(discover_by_cv_person(&state, params.cv_person_id).await?))
}
```

- [ ] **Step 9: Register the routes**

In the `router()` fn in `longbox-web/src/routes/creators.rs`, add the two routes. Put `/creators/cv-search` and `/creators/discover` BEFORE `/creators/:id` so the static segments are not shadowed by the `:id` param (Axum matches static over dynamic, but keep them grouped for clarity):

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/creators/search", get(search_handler))
        .route("/creators/cv-search", get(cv_search_handler))
        .route("/creators/discover", get(discover_cv_handler))
        .route("/creators/:id", get(detail_handler))
        .route("/creators/:id/issues", get(issues_handler))
        .route("/creators/:id/discover", get(discover))
}
```

- [ ] **Step 10: Verify build, lint, tests**

Run: `SQLX_OFFLINE=true cargo clippy -p longbox-web --all-targets -- -D warnings && SQLX_OFFLINE=true cargo test -p longbox-web`
Expected: clean; the new `flag_candidates` test and existing `discover_tests` pass.

- [ ] **Step 11: Commit**

```bash
git add longbox-db/src/creator_repo.rs longbox-web/src/routes/creators.rs .sqlx
git commit -m "feat(web): CV creator search + discover-by-cv_person_id routes"
```

---

## Task 3: Frontend — ComicVine fallback section + shared discovery component

**Files:**
- Modify: `longbox-frontend/src/lib/api/creators.ts`
- Create: `longbox-frontend/src/lib/components/CreatorDiscovery.svelte`
- Modify: `longbox-frontend/src/routes/creators/[id]/+page.svelte`
- Modify: `longbox-frontend/src/routes/creators/+page.svelte`

- [ ] **Step 1: Add API types + fetchers**

In `longbox-frontend/src/lib/api/creators.ts`, after the existing `getCreatorDiscovery` (end of file), add:

```typescript
export interface CvCreatorCandidate {
  cv_person_id: number;
  name: string;
  description: string | null;
  country: string | null;
  image_url: string | null;
  in_library_creator_id: number | null;
}

export function searchCvCreators(q: string): Promise<CvCreatorCandidate[]> {
  return apiFetch(`/creators/cv-search?q=${encodeURIComponent(q)}`);
}

export function discoverByCvPerson(cvPersonId: number): Promise<DiscoveredVolume[]> {
  return apiFetch(`/creators/discover?cv_person_id=${cvPersonId}`);
}
```

- [ ] **Step 2: Create the shared discovery component**

Create `longbox-frontend/src/lib/components/CreatorDiscovery.svelte`. It renders an already-loaded `DiscoveredVolume[]` as owned/not-owned lists and owns the acquire state internally (extracted verbatim from the current `[id]/+page.svelte` discovery markup + `acquire` logic):

```svelte
<script lang="ts">
  import { type DiscoveredVolume } from '$lib/api/creators';
  import { addSeries } from '$lib/api/series';
  import Button from '$lib/components/Button.svelte';

  let { volumes }: { volumes: DiscoveredVolume[] } = $props();

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
</script>

{#if notInLibrary.length > 0}
  <h3 class="mt-3 text-sm font-semibold text-slate-600">Not in your library</h3>
  <ul class="space-y-1">
    {#each notInLibrary as v (v.cv_volume_id)}
      <li class="flex items-center justify-between gap-2">
        <span>{v.name}</span>
        {#if addedIds.has(v.cv_volume_id)}
          <span class="text-sm text-green-600">Added</span>
        {:else}
          <Button
            loading={addingId === v.cv_volume_id}
            onclick={() => acquire(v.cv_volume_id)}
          >Acquire</Button>
        {/if}
      </li>
    {/each}
  </ul>
{/if}

{#if inLibrary.length > 0}
  <h3 class="mt-3 text-sm font-semibold text-slate-600">Already in your library</h3>
  <ul class="space-y-1">
    {#each inLibrary as v (v.cv_volume_id)}
      <li><a href={`/series/${v.series_id}`} class="hover:underline">{v.name}</a></li>
    {/each}
  </ul>
{/if}

{#if volumes.length === 0}
  <p class="mt-2 text-sm text-slate-500">No series found for this creator.</p>
{/if}
```

If `Button.svelte` does not accept a `loading` prop, check its props first (`longbox-frontend/src/lib/components/Button.svelte`) and match the existing usage from `[id]/+page.svelte` — reuse the exact same prop names that page already passes.

- [ ] **Step 3: Use the shared component in the creator-detail page**

In `longbox-frontend/src/routes/creators/[id]/+page.svelte`, replace the inline discovery markup (the `notInLibrary` / `inLibrary` `{#each}` blocks) and the local `acquire`/`addingId`/`addedIds`/`inLibrary`/`notInLibrary` state with the shared component. Keep the "Discover more by X" button + `loadDiscovery` loader. After discovery loads, render:

```svelte
{#if discovery !== null}
  <CreatorDiscovery volumes={discovery} />
{/if}
```

Add the import at the top: `import CreatorDiscovery from '$lib/components/CreatorDiscovery.svelte';` and remove the now-unused `addSeries` / `Button` imports and the moved state/`acquire` fn (leave `getCreatorDiscovery`, `discovery`, `discovering`, `discoverError`, `loadDiscovery`).

- [ ] **Step 4: Add the ComicVine fallback section to the creators index page**

In `longbox-frontend/src/routes/creators/+page.svelte`, extend the existing debounced search to ALSO query CV, and render a fallback section. Replace the `<script>` block's `onInput` and add CV state; then add the section after the in-library `<ul>`:

```svelte
<script lang="ts">
  import {
    searchCreators, searchCvCreators, discoverByCvPerson,
    type CreatorSearchRow, type CvCreatorCandidate, type DiscoveredVolume,
  } from '$lib/api/creators';
  import CreatorDiscovery from '$lib/components/CreatorDiscovery.svelte';

  let q = $state('');
  let results = $state<CreatorSearchRow[]>([]);
  let cvResults = $state<CvCreatorCandidate[]>([]);
  let timer = $state<ReturnType<typeof setTimeout> | undefined>(undefined);
  let loading = $state(false);

  // Which CV candidate is expanded, and its loaded bibliography.
  let expanded = $state<number | null>(null);
  let expandedVolumes = $state<DiscoveredVolume[] | null>(null);
  let expandLoading = $state(false);

  function onInput() {
    clearTimeout(timer);
    const term = q.trim();
    expanded = null; expandedVolumes = null;
    if (term.length < 2) { results = []; cvResults = []; return; }
    timer = setTimeout(async () => {
      loading = true;
      try {
        [results, cvResults] = await Promise.all([
          searchCreators(term),
          searchCvCreators(term).catch(() => []),
        ]);
      } finally { loading = false; }
    }, 300);
  }

  async function toggleDiscover(cvPersonId: number) {
    if (expanded === cvPersonId) { expanded = null; expandedVolumes = null; return; }
    expanded = cvPersonId;
    expandedVolumes = null;
    expandLoading = true;
    try {
      expandedVolumes = await discoverByCvPerson(cvPersonId);
    } finally {
      expandLoading = false;
    }
  }
</script>

<h1 class="mb-4 text-2xl font-bold">Creators</h1>
<input
  type="search"
  class="mb-4 w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm shadow-sm focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
  placeholder="Search creators…"
  bind:value={q}
  oninput={onInput}
/>
{#if loading}<p class="text-sm text-slate-500">Searching…</p>{/if}

{#if results.length > 0}
  <h2 class="mb-1 text-sm font-semibold text-slate-600">In your library</h2>
  <ul class="space-y-1">
    {#each results as c (c.id)}
      <li class="flex items-baseline gap-2">
        <a href={`/creators/${c.id}`} class="font-medium hover:underline">{c.name}</a>
        <span class="text-sm text-slate-500">{c.issue_count} issues · {c.series_count} series</span>
      </li>
    {/each}
  </ul>
{/if}

{#if cvResults.length > 0}
  <h2 class="mb-1 mt-5 text-sm font-semibold text-slate-600">Not in your library · ComicVine</h2>
  <ul class="space-y-3">
    {#each cvResults as p (p.cv_person_id)}
      <li class="rounded-md border border-slate-200 p-3">
        <div class="flex items-start gap-3">
          {#if p.image_url}
            <img src={p.image_url} alt={p.name} class="h-12 w-12 shrink-0 rounded object-cover" />
          {/if}
          <div class="min-w-0 flex-1">
            <div class="flex items-baseline gap-2">
              <span class="font-medium">{p.name}</span>
              {#if p.country}<span class="text-xs text-slate-400">{p.country}</span>{/if}
            </div>
            {#if p.description}<p class="text-sm text-slate-500">{p.description}</p>{/if}
            <div class="mt-1">
              {#if p.in_library_creator_id !== null}
                <a href={`/creators/${p.in_library_creator_id}`} class="text-sm text-blue-600 hover:underline">
                  already in your library ↗
                </a>
              {:else}
                <button
                  class="text-sm text-blue-600 hover:underline"
                  onclick={() => toggleDiscover(p.cv_person_id)}
                >{expanded === p.cv_person_id ? 'Hide' : 'Discover ▾'}</button>
              {/if}
            </div>
            {#if expanded === p.cv_person_id}
              {#if expandLoading}
                <p class="mt-2 text-sm text-slate-500">Loading bibliography…</p>
              {:else if expandedVolumes !== null}
                <CreatorDiscovery volumes={expandedVolumes} />
              {/if}
            {/if}
          </div>
        </div>
      </li>
    {/each}
  </ul>
{/if}

{#if !loading && q.trim().length >= 2 && results.length === 0 && cvResults.length === 0}
  <p class="text-sm text-slate-500">No creators found.</p>
{/if}
```

- [ ] **Step 5: Verify the frontend builds (the CI gate)**

Run: `cd longbox-frontend && pnpm build`
Expected: build succeeds. (Do NOT rely on `pnpm check`/vitest — pre-existing unrelated failures; `pnpm build` is the CI gate.)

- [ ] **Step 6: Restore the frontend-dist placeholder if a local build touched it**

Run: `git checkout HEAD -- longbox-web/frontend-dist/index.html 2>/dev/null; git status --short longbox-web/frontend-dist`
Expected: no staged bundle artifacts (the tracked `index.html` is a placeholder; `_app/` + `favicon.ico` are gitignored).

- [ ] **Step 7: Commit**

```bash
git add longbox-frontend/src/lib/api/creators.ts longbox-frontend/src/lib/components/CreatorDiscovery.svelte longbox-frontend/src/routes/creators/[id]/+page.svelte longbox-frontend/src/routes/creators/+page.svelte
git commit -m "feat(frontend): ComicVine creator-search fallback + shared discovery component"
```

---

## Final Verification (after all tasks)

- [ ] **Full workspace gate (mirrors CI):**

```bash
SQLX_OFFLINE=true cargo fmt --all -- --check
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
SQLX_OFFLINE=true cargo test --workspace
(cd longbox-frontend && pnpm build)
```
Expected: fmt clean, clippy 0 warnings, all tests pass, frontend build succeeds.

- [ ] **Manual smoke (after deploy):** On `/creators`, type a creator you own nothing by (e.g. "Fiona Staples"). Confirm: in-library section (if any) shows first; a "Not in your library · ComicVine" section lists ranked CV people with deck/country/thumb; a person already owned shows the "already in your library ↗" link; clicking "Discover ▾" on a not-owned person loads their bibliography with Acquire buttons; Acquire adds the series (folder + auto-pull-search).

---

## Self-Review

**Spec coverage:**
- CV person search (arbitrary, incl. unowned) → Task 1 (`search_persons`) + Task 2 (`cv_search_handler`). ✅
- Discover by raw cv_person_id, reuse `build_discovery` → Task 2 (`discover_by_cv_person`, `discover_cv_handler`). ✅
- Ephemeral (no DB write, no migration) → confirmed: no migration task; only a read query (`cv_person_id_map`). ✅
- Cross-link owned creators → Task 2 (`flag_candidates` + `in_library_creator_id`), Task 3 (badge + link). ✅
- Display name+deck+country+thumb, CV rank, cap ~12 → Task 3 markup; cap is CV's `PAGE_LIMIT` (Task 1 reuses it). ✅
- Auto-fallback section UX → Task 3 (`/creators` page, parallel search). ✅
- Acquire reuses `POST /api/series {cv_id}` → Task 3 (`addSeries` in shared component). ✅

**Placeholder scan:** No TBD/TODO; every code step shows full code. Two conditional checks flagged inline (Button `loading` prop, sqlx prepare fallback path) with concrete instructions, not placeholders.

**Type consistency:** `CvPersonSearchResult` (comicvine) → `CvCreatorCandidate` (web) → `CvCreatorCandidate` (ts) field names match (`cv_person_id`, `description`, `country`, `image_url`, `in_library_creator_id`). `DiscoveredVolume` reused unchanged across `discover` and `discover_cv_handler`. `discoverByCvPerson` hits `/creators/discover?cv_person_id=` matching `DiscoverCvParams`. `flag_candidates` signature identical in test (Step 3) and impl (Step 5).
