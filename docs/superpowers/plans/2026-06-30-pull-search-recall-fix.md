# Pull-Search Recall Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three defects that make LongBox's pull-search return zero hits for titles it should find (colon/acronym over-specification, missing CV aliases, unrequested publisher/imprint tokens), in one coherent pass through the normalize / query-builder / match-filter layer.

**Architecture:** The query builder currently sends the raw series title (colon and all) as a single AND-tokenized Newznab `q`, while the match filter strips that same punctuation — so the query over-specifies and the filter never sees a hit. We (a) normalize the query term and add a documented relaxation ladder (full → colon-split → alias → title-only), (b) make the match filter tolerant of hyphen-splitting and known publisher/imprint tokens and aware of aliases, (c) add an exact issue-number gate so relaxed queries can't grab the wrong issue, and (d) capture CV's `aliases` field into a new `series.aliases` column read only in the pull path.

**Tech Stack:** Rust, sqlx (SQLite, offline `.sqlx` metadata), tokio, the `longbox-newznab` / `longbox-core` / `longbox-comicvine` / `longbox-cv-enrichment` / `longbox-db` / `longbox-pull` crates.

**Locked kickoff decisions:**
1. Issue-number gate = exact normalized match only (reuse `IssueNumber::matches`).
2. Relaxation ladder = colon-split + alias rungs only (NO generic drop-first-token — deferred to GH #12).
3. Publisher token list = static curated imprint list, strip from both sides.
4. Alias search = only on zero hits from primary (ladder is sequential stop-on-first-raw-nonempty, so this is automatic).

**Pre-flight (run once before Task 1):**
```bash
git checkout -b fix/pull-search-recall
cargo test -p longbox-newznab -p longbox-core   # baseline: should pass
```

**sqlx note:** Tasks 7–9 change/add compile-checked queries. After each such change, regenerate offline metadata or the workspace won't build outside a live DB:
```bash
# from repo root, with a throwaway migrated sqlite:
export DATABASE_URL="sqlite:/tmp/lb-prepare.db?mode=rwc"
cargo sqlx database create && cargo sqlx migrate run --source longbox-db/migrations
cargo sqlx prepare --workspace
```
If `cargo sqlx` is unavailable, install with `cargo install sqlx-cli --no-default-features --features sqlite`.

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `longbox-newznab/src/publisher.rs` (create) | Static imprint/publisher token list + strip helper | 1 |
| `longbox-newznab/src/select.rs` (modify) | Hyphen-split + publisher-strip + alias-aware similarity; issue-number gate | 1,2,3,4 |
| `longbox-newznab/src/query.rs` (modify) | Query-term normalization + variant builders | 5 |
| `longbox-newznab/src/client.rs` (modify) | Relaxation ladder; thread aliases + issue into filter | 3,4,6 |
| `longbox-newznab/src/lib.rs` (modify) | Export new `publisher` module items as needed | 1 |
| `longbox-comicvine/src/models.rs` (modify) | `aliases` on `CvVolumeFull` | 8 |
| `longbox-comicvine/src/projection.rs` (modify) | `aliases` on `CvVolumeDetail` + `project_volume` | 8 |
| `longbox-db/migrations/20260630120000_add_series_aliases.sql` (create) | `series.aliases` column | 7 |
| `longbox-db/src/series_repo.rs` (modify) | `update_series_volume_detail` writes aliases; new `get_aliases` reader | 7,9 |
| `longbox-cv-enrichment/src/worker.rs` (modify) | Pass aliases into the volume-detail writer | 9 |
| `longbox-pull/src/engine.rs` (modify) | Fetch aliases + pass into `find_release_excluding_filtered` | 10 |

---

## Commit 1 — Publisher/imprint tolerance + hyphen agreement

### Task 1: Static publisher/imprint token list + strip helper

**Files:**
- Create: `longbox-newznab/src/publisher.rs`
- Modify: `longbox-newznab/src/lib.rs` (add `mod publisher;`)

- [ ] **Step 1: Write the failing test**

Create `longbox-newznab/src/publisher.rs` with only the test module first:

```rust
//! Static curated publisher/imprint token list. Scene/Usenet release names
//! routinely prepend the publisher or imprint as a literal token
//! ("Vertigo-Fbp...", "DC.K.O..."), and some catalog titles carry it too
//! ("DC K.O.", "EC Cruel Universe"). The match filter strips these tokens
//! from BOTH the requested title and the parsed release title so an
//! unrequested imprint token doesn't dilute similarity.
//!
//! Deliberately a static list, not derived from `series.publisher`: that
//! column is parent-level ("DC Comics") not scene-level ("Vertigo"), and
//! using real data as a token source reintroduces the fragile-heuristic
//! problem the similarity filter already has. Missing imprint → one-line
//! addition here when it surfaces (see GH #12 follow-up rationale).

/// Lowercase, single-word imprint/publisher tokens tolerated as noise.
/// Multi-word publishers are listed token-by-token (e.g. "dark", "horse").
/// Keep entries lowercase — matching happens in normalized (lowercased) space.
const PUBLISHER_TOKENS: &[&str] = &[
    "marvel", "dc", "image", "vertigo", "darkhorse", "dark", "horse",
    "boom", "idw", "dynamite", "valiant", "titan", "oni", "madcave", "mad",
    "cave", "vault", "aftershock", "ablaze", "ec", "archie", "dstlry",
    "skybound", "wildstorm", "milestone", "blackmask",
];

/// Remove leading/trailing publisher tokens from a normalized,
/// space-separated token string. Only strips tokens at the EDGES of the
/// string so an internal word that happens to collide (rare) is preserved;
/// scene names put the imprint at the front, occasionally the back.
/// Never strips the last remaining token (so a title that IS just a
/// publisher word can't normalize to empty).
pub fn strip_publisher_tokens(normalized: &str) -> String {
    let mut tokens: Vec<&str> = normalized.split_whitespace().collect();
    // strip from front
    while tokens.len() > 1 && PUBLISHER_TOKENS.contains(&tokens[0]) {
        tokens.remove(0);
    }
    // strip from back
    while tokens.len() > 1 && PUBLISHER_TOKENS.contains(tokens.last().unwrap()) {
        tokens.pop();
    }
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_imprint_token() {
        assert_eq!(
            strip_publisher_tokens("vertigo fbp federal bureau of physics"),
            "fbp federal bureau of physics"
        );
    }

    #[test]
    fn strips_leading_publisher_from_catalog_title() {
        // "DC K.O." normalized is "dc k o"; "EC Cruel Universe" -> "ec cruel universe".
        assert_eq!(strip_publisher_tokens("dc k o"), "k o");
        assert_eq!(strip_publisher_tokens("ec cruel universe"), "cruel universe");
    }

    #[test]
    fn preserves_internal_and_non_publisher_tokens() {
        assert_eq!(strip_publisher_tokens("saga"), "saga");
        assert_eq!(
            strip_publisher_tokens("federal bureau of physics"),
            "federal bureau of physics"
        );
    }

    #[test]
    fn never_strips_to_empty() {
        // A title that is only a publisher word survives intact.
        assert_eq!(strip_publisher_tokens("vertigo"), "vertigo");
    }
}
```

- [ ] **Step 2: Wire the module**

In `longbox-newznab/src/lib.rs`, add alongside the other `mod` declarations:

```rust
mod publisher;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p longbox-newznab publisher::`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
git add longbox-newznab/src/publisher.rs longbox-newznab/src/lib.rs
git commit -m "feat(search): static publisher/imprint token strip helper"
```

### Task 2: Hyphen-split + publisher-strip in the similarity comparison

**Files:**
- Modify: `longbox-newznab/src/select.rs` (add a `match_normalize` helper; use it in `filter_by_series_title` at the two `normalize_title(...)` call sites — `select.rs:384` and `select.rs:434` — and in `score_release_title` at `select.rs:494-495`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `longbox-newznab/src/select.rs`:

```rust
#[test]
fn publisher_prefixed_release_still_matches() {
    let patterns = default_patterns();
    // Hyphen-joined imprint prefix ("Vertigo-Fbp") that previously diluted
    // Jaccard below 0.75 now scores high after match-normalize.
    let score = score_release_title(
        "Vertigo-Fbp.Federal.Bureau.Of.Physics.005.2013.digital.Zone-Empire",
        "FBP: Federal Bureau of Physics",
        &patterns,
    )
    .unwrap();
    assert!(score >= 0.75, "publisher-prefixed should match, got {score}");
}

#[test]
fn hyphen_variants_score_equal() {
    let patterns = default_patterns();
    // "Spider.Man" (scene dotted) vs catalog "Spider-Man": hyphen split makes
    // them token-equal instead of {spider-man} vs {spider, man}.
    let score = score_release_title("Spider.Man.005.2023.digital.X-Empire", "Spider-Man", &patterns)
        .unwrap();
    assert!(score >= 0.9, "hyphen variants should match high, got {score}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p longbox-newznab publisher_prefixed_release_still_matches hyphen_variants_score_equal`
Expected: FAIL (`publisher_prefixed...` scores ~0.667; the hyphen test may already pass via Levenshtein — both must pass after the change).

- [ ] **Step 3: Add the `match_normalize` helper**

In `longbox-newznab/src/select.rs`, just above `filter_by_series_title` (before `select.rs:358`), add:

```rust
/// Normalize a title for similarity comparison: run the shared
/// `normalize_title` (lowercase, punctuation→space except hyphen, article
/// strip), THEN split hyphens to spaces and strip edge publisher/imprint
/// tokens. Hyphen-splitting makes "spider-man" / "spider man" / "vertigo-fbp"
/// tokenize identically on both sides (the query and match layers must agree
/// on hyphen as a separator); publisher-strip removes an unrequested imprint
/// token so it can't dilute the token-set Jaccard score. Kept local to the
/// newznab matcher — `normalize_title` itself must keep hyphens for
/// `sort_title` identity, so we layer the extra steps here only.
fn match_normalize(input: &str) -> String {
    let base = normalize_title(input).replace('-', " ");
    let collapsed: String = base.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::publisher::strip_publisher_tokens(&collapsed)
}
```

- [ ] **Step 4: Use it at the three comparison sites**

In `filter_by_series_title`, replace `select.rs:384`:

```rust
let requested_normalized = normalize_title(requested_series_title);
```
with:
```rust
let requested_normalized = match_normalize(requested_series_title);
```

Replace `select.rs:434`:
```rust
let parsed_normalized = normalize_title(&parsed.series_title);
```
with:
```rust
let parsed_normalized = match_normalize(&parsed.series_title);
```

In `score_release_title`, replace `select.rs:494-495`:
```rust
let requested_normalized = normalize_title(requested_series_title);
let parsed_normalized = normalize_title(&parsed.series_title);
```
with:
```rust
let requested_normalized = match_normalize(requested_series_title);
let parsed_normalized = match_normalize(&parsed.series_title);
```

- [ ] **Step 5: Run to verify pass (and no regressions)**

Run: `cargo test -p longbox-newznab`
Expected: PASS, including the two new tests and all existing `filter_by_series_title` / `score_release_title` tests.

- [ ] **Step 6: Commit**

```bash
git add longbox-newznab/src/select.rs
git commit -m "fix(search): hyphen-split + publisher-strip in match similarity"
```

---

## Commit 2 — Exact issue-number gate

### Task 3: Issue-number gate in `filter_by_series_title`

**Files:**
- Modify: `longbox-newznab/src/select.rs` (`filter_by_series_title` signature + gate loop)
- Modify: `longbox-newznab/src/client.rs` (the one production caller, `find_release_excluding_filtered` at `client.rs:349`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `longbox-newznab/src/select.rs` (the existing tests build `Release` via the `release(...)` helper and call `filter_by_series_title` — match that shape, adding the new `requested_issue` arg as the LAST argument):

```rust
#[test]
fn issue_number_gate_drops_wrong_issue() {
    let patterns = default_patterns();
    let pool = vec![
        release("Saga 005 (2012) (digital).cbz", Some(10)),
        release("Saga 012 (2012) (digital).cbz", Some(10)),
    ];
    let outcome = filter_by_series_title(
        pool,
        &patterns,
        "Saga",
        None,        // requested_year
        0.75,        // threshold
        &[],         // exclusion_keywords
        None,        // min_size_bytes
        "5",         // requested_issue (NEW, last arg) — "5" matches "005"
    );
    assert_eq!(outcome.kept.len(), 1);
    assert_eq!(outcome.kept[0].title, "Saga 005 (2012) (digital).cbz");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p longbox-newznab issue_number_gate_drops_wrong_issue`
Expected: FAIL to compile (`filter_by_series_title` takes 7 args, test passes 8).

- [ ] **Step 3: Add the parameter and gate**

In `longbox-newznab/src/select.rs`, change the `filter_by_series_title` signature (`select.rs:358-366`) to add a final parameter:

```rust
pub fn filter_by_series_title(
    releases: Vec<Release>,
    patterns: &[ParsingPattern],
    requested_series_title: &str,
    requested_year: Option<i32>,
    threshold: f64,
    exclusion_keywords: &[String],
    min_size_bytes: Option<i64>,
    requested_issue: &str,
) -> FilterOutcome {
```

Add the import at the top of `select.rs` (near the other `longbox_core` use, `select.rs:22`):

```rust
use longbox_core::issue::IssueNumber;
```

In the per-release loop, immediately AFTER the similarity `continue` (after `select.rs:444`, i.e. after the `if score < threshold { continue; }` block) and BEFORE the year gate, add the exact issue gate:

```rust
        // Issue-number gate (exact, zero-pad tolerant). Load-bearing for the
        // relaxed query rungs (colon-split / alias / title-only) which return
        // the whole series — without this an off-issue release could be
        // grabbed. `IssueNumber::matches` treats "5" == "005" == "5" but
        // distinguishes "5AU"/"Annual 5".
        if !IssueNumber::new(requested_issue).matches(&IssueNumber::new(parsed.number.clone())) {
            continue;
        }
```

- [ ] **Step 4: Update the production caller**

In `longbox-newznab/src/client.rs`, the `find_release_excluding_filtered` call to `filter_by_series_title` (`client.rs:349-357`) passes `series, year, ...`. Add `issue` as the final argument:

```rust
                let outcome = filter_by_series_title(
                    kept_pre_filter,
                    patterns,
                    series,
                    year,
                    similarity_threshold,
                    exclusion_keywords,
                    min_size_bytes,
                    issue,
                );
```

(`issue: &str` is already a parameter of `find_release_excluding_filtered`, `client.rs:295`.)

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p longbox-newznab`
Expected: PASS. Existing `filter_by_series_title_*` tests now need the extra arg — fix any that fail to compile by appending a matching `requested_issue` argument (use the issue number embedded in each test's release titles, or `""` where the test pool is single-issue and issue-agnostic; `""` matches nothing, so for existing tests that assert kept-non-empty, pass the actual issue number from the release title).

- [ ] **Step 6: Commit**

```bash
git add longbox-newznab/src/select.rs longbox-newznab/src/client.rs
git commit -m "feat(search): exact issue-number gate in pre-grab filter"
```

---

## Commit 3 — Alias-aware filter

### Task 4: Score releases against primary title OR any alias

**Files:**
- Modify: `longbox-newznab/src/select.rs` (`filter_by_series_title` takes `aliases`, score = max over primary+aliases)
- Modify: `longbox-newznab/src/client.rs` (`find_release_excluding_filtered` takes + threads `aliases`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `longbox-newznab/src/select.rs`:

```rust
#[test]
fn alias_only_titled_release_matches() {
    let patterns = default_patterns();
    // FBP's original title was "Collider"; an alias-ONLY scene release names
    // just "Collider 001". Scores ~0.1 against the primary title; matches via
    // the alias candidate (1.0).
    let pool = vec![release("Collider.001.2013.digital.Zone-Empire", Some(10))];
    let outcome = filter_by_series_title(
        pool,
        &patterns,
        "FBP: Federal Bureau of Physics",
        None,
        0.75,
        &[],
        None,
        "1",
        &["Collider".to_string()],   // aliases (NEW, last arg)
    );
    assert_eq!(outcome.kept.len(), 1, "alias-only release should be kept");
}

#[test]
fn alias_embedded_in_full_title_release_matches() {
    let patterns = default_patterns();
    // The user's MOTIVATING Problem-2 case: the real original-title release
    // carries BOTH the current title and the old one —
    // "Federal.Bureau.of.Physics.Collider.001". Against the primary this is
    // 4/6 = 0.667 Jaccard (the extra "collider" token dilutes), and 0.2
    // against the bare alias — BOTH below 0.75. It matches only once alias
    // tokens are stripped from the release before scoring against the
    // primary: "federal bureau of physics collider" - "collider" ->
    // "federal bureau of physics" -> 4/5 = 0.8 vs "fbp federal bureau of
    // physics".
    let pool = vec![release(
        "Federal.Bureau.of.Physics.Collider.001.2013.digital.Zone-Empire",
        Some(10),
    )];
    let outcome = filter_by_series_title(
        pool,
        &patterns,
        "FBP: Federal Bureau of Physics",
        None,
        0.75,
        &[],
        None,
        "1",
        &["Collider".to_string()],
    );
    assert_eq!(outcome.kept.len(), 1, "alias-embedded release should be kept");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p longbox-newznab alias_titled_release_matches_when_alias_supplied`
Expected: FAIL to compile (arity).

- [ ] **Step 3: Add `aliases` and use max-over-titles scoring**

In `longbox-newznab/src/select.rs`, append a parameter to `filter_by_series_title`:

```rust
    requested_issue: &str,
    aliases: &[String],
) -> FilterOutcome {
```

Replace the requested-normalized line (now `match_normalize(requested_series_title)` from Task 2) with a precomputed vector of all candidate normalized titles, just before the loop:

```rust
    // All titles a release may legitimately match: the primary plus any known
    // alias (e.g. CV's "Collider" for FBP). A release scores against the BEST
    // of these — alias-titled scene releases otherwise fail similarity against
    // the primary title even after the alias query surfaced them.
    let candidate_titles: Vec<String> = std::iter::once(requested_series_title)
        .chain(aliases.iter().map(String::as_str))
        .map(match_normalize)
        .filter(|t| !t.is_empty())
        .collect();

    // Alias tokens are ALSO strippable noise on the release side. Scene
    // releases of a renamed series carry BOTH titles ("Federal Bureau of
    // Physics Collider"); the extra alias token dilutes Jaccard against the
    // primary below threshold. Stripping alias tokens from the release before
    // scoring against the primary recovers these — the user's motivating
    // Problem-2 case. Same noise-token shape as the publisher strip.
    let alias_tokens: std::collections::HashSet<String> = aliases
        .iter()
        .flat_map(|a| {
            match_normalize(a)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
```

Replace the per-release score computation (the `let parsed_normalized = ...; let score = similarity(...);` at `select.rs:434-435`, already `match_normalize` from Task 2) with the two-form scoring — full release form (catches alias-ONLY releases via the alias candidate) and alias-stripped form (catches alias-EMBEDDED releases via the primary candidate), each scored against every candidate, best wins:

```rust
        let parsed_normalized = match_normalize(&parsed.series_title);
        // Alias-stripped variant: drop any alias tokens so an alias embedded
        // alongside the real title doesn't dilute the primary match. Equals
        // `parsed_normalized` when there are no aliases (backward-compatible).
        let parsed_dealiased: String = parsed_normalized
            .split_whitespace()
            .filter(|t| !alias_tokens.contains(*t))
            .collect::<Vec<_>>()
            .join(" ");
        let score = candidate_titles
            .iter()
            .flat_map(|cand| {
                [
                    similarity(cand, &parsed_normalized),
                    similarity(cand, &parsed_dealiased),
                ]
            })
            .fold(0.0_f64, f64::max);
```

Delete the now-unused `requested_normalized` binding if Task 2 left one (the `candidate_titles` vec replaces it). The `MismatchDiagnostic` `into_error_message` still takes `requested_series_title` from the caller — unchanged. When `aliases` is empty, `alias_tokens` is empty and `parsed_dealiased == parsed_normalized`, so the scoring reduces exactly to the prior single-form behavior.

- [ ] **Step 4: Thread `aliases` through the caller**

In `longbox-newznab/src/client.rs`, add `aliases: &[String]` to `find_release_excluding_filtered`'s signature (after `cover_date`, before `min_size_bytes` is fine — pick a position and keep it consistent; append after `min_size_bytes` to minimize churn):

```rust
    cover_date: Option<&str>,
    min_size_bytes: Option<i64>,
    aliases: &[String],
) -> Result<FindOutcome, NewznabError> {
```

Pass it into the `filter_by_series_title` call (extend Task 3's call):

```rust
                    issue,
                    aliases,
                );
```

- [ ] **Step 5: Update in-crate callers/tests**

The crate tests `find_release_excluding_filtered_*` (`client.rs:610`, `client.rs:656`) now need the extra arg — append `&[]` (no aliases) to each call.

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p longbox-newznab`
Expected: PASS including the new alias test.

- [ ] **Step 7: Commit**

```bash
git add longbox-newznab/src/select.rs longbox-newznab/src/client.rs
git commit -m "feat(search): alias-aware similarity in pre-grab filter"
```

---

## Commit 4 — Query normalization + relaxation ladder

### Task 5: Query-term normalization + variant builders

**Files:**
- Modify: `longbox-newznab/src/query.rs` (new `normalize_query`, `title_variants`; `build_search_term` normalizes)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `longbox-newznab/src/query.rs`:

```rust
#[test]
fn search_term_is_query_normalized() {
    // Colon dropped, lowercased, collapsed — the q the indexer AND-matches
    // no longer carries "fbp:" (a token no release contains).
    assert_eq!(
        build_search_term("FBP: Federal Bureau of Physics", "5", true),
        "fbp federal bureau of physics 005"
    );
}

#[test]
fn title_variants_yields_full_then_colon_splits() {
    // Order matters: full first, then after-colon, then before-colon.
    let v = title_variants("FBP: Federal Bureau of Physics");
    assert_eq!(
        v,
        vec![
            "FBP: Federal Bureau of Physics".to_string(),
            "Federal Bureau of Physics".to_string(),
            "FBP".to_string(),
        ]
    );
}

#[test]
fn title_variants_no_colon_is_single() {
    assert_eq!(title_variants("Saga"), vec!["Saga".to_string()]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p longbox-newznab -- query::tests::search_term_is_query_normalized query::tests::title_variants`
Expected: FAIL (`title_variants` undefined; `build_search_term` returns un-normalized).

- [ ] **Step 3: Add `normalize_query` and `title_variants`; normalize in `build_search_term`**

In `longbox-newznab/src/query.rs`, add near the top (after the imports):

```rust
/// Aggressively normalize a title into a Newznab `q` term. Indexers
/// AND-tokenize `q` and Prowlarr forwards it verbatim (no CleanTitle), so
/// every punctuation char that a scene release name omits is a recall cut.
/// Lowercase, map ALL non-alphanumeric (colon, hyphen, dot, paren, slash,
/// ampersand…) to space, collapse whitespace. Hyphen→space here is the
/// query-side half of the hyphen agreement (`select::match_normalize` is the
/// match-side half).
pub fn normalize_query(s: &str) -> String {
    let lowered: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Title variants for the relaxation ladder, most-specific first:
/// 1. the full title (as given), 2. the substring AFTER the first colon,
/// 3. the substring BEFORE the first colon. Colon-less titles yield just
/// `[full]`. Variants are returned RAW (not query-normalized) so callers can
/// reuse them as match-side aliases if desired; `build_search_term` /
/// `build_url` normalize at send time. Empty/whitespace splits are dropped.
pub fn title_variants(title: &str) -> Vec<String> {
    let mut out = vec![title.to_string()];
    if let Some(idx) = title.find(':') {
        let after = title[idx + 1..].trim();
        let before = title[..idx].trim();
        if !after.is_empty() {
            out.push(after.to_string());
        }
        if !before.is_empty() {
            out.push(before.to_string());
        }
    }
    out
}
```

Change `build_search_term` (`query.rs:37-47`) to normalize the series before formatting (issue padding logic unchanged, but apply to the normalized issue too):

```rust
pub fn build_search_term(series: &str, issue: &str, padded: bool) -> String {
    let series_q = normalize_query(series);
    let issue_part = if padded {
        match issue.parse::<u32>() {
            Ok(n) => format!("{n:03}"),
            Err(_) => normalize_query(issue),
        }
    } else {
        normalize_query(issue)
    };
    format!("{series_q} {issue_part}")
}
```

- [ ] **Step 4: Fix the existing `build_search_term` tests**

The existing tests at `query.rs:140-183` assert mixed-case / colon-bearing output (e.g. `"Wolverine 005"`). Update their expectations to the normalized form: `"wolverine 005"`, `"saga 012"`, `"bone annual 1"`, `"promethea"` (the `½` char is non-alphanumeric → dropped, so `build_search_term("Promethea", "½", true)` becomes `"promethea"` with a trailing-space-collapsed empty issue — assert `"promethea"`). Update `year_is_never_embedded_in_the_query_term` expectations to lowercased forms. Update `url_carries_all_required_params` (`query.rs:196`) `q=` expectation to `q=wolverine+005`.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p longbox-newznab -- query::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add longbox-newznab/src/query.rs
git commit -m "feat(search): normalize query term + colon-split title variants"
```

### Task 6: Relaxation ladder in `search_one_indexer`

**Files:**
- Modify: `longbox-newznab/src/client.rs` (`search_one_indexer` becomes a ladder; thread `aliases`)

- [ ] **Step 1: Write the failing test**

`search_one_indexer` is private and hits the network. Test the ladder's term sequence purely instead. Add to `longbox-newznab/src/query.rs` a pure helper the ladder will consume, plus its test (this keeps the ladder logic testable without HTTP):

```rust
/// The ordered list of `q` search terms to try for an issue, most-specific
/// first, stopping at the first that returns hits (the caller does the
/// stopping). Order: full title (padded, unpadded) → each colon-split variant
/// (padded, unpadded) → each alias (padded, unpadded) → title-only (no issue).
/// De-duplicated, preserving first occurrence. The title-only rung relies on
/// the downstream exact issue-number gate to avoid wrong-issue grabs.
pub fn search_ladder(series: &str, issue: &str, aliases: &[String]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut push = |t: String| {
        if !t.trim().is_empty() && !terms.contains(&t) {
            terms.push(t);
        }
    };
    for variant in title_variants(series) {
        push(build_search_term(&variant, issue, true));
        push(build_search_term(&variant, issue, false));
    }
    for alias in aliases {
        push(build_search_term(alias, issue, true));
        push(build_search_term(alias, issue, false));
    }
    // title-only recall rung (no issue token); gated downstream by issue match.
    push(normalize_query(series));
    terms
}
```

Test:

```rust
#[test]
fn search_ladder_orders_specific_to_general() {
    let ladder = search_ladder(
        "FBP: Federal Bureau of Physics",
        "1",
        &["Collider".to_string()],
    );
    // full padded first, title-only last.
    assert_eq!(ladder.first().unwrap(), "fbp federal bureau of physics 001");
    assert_eq!(ladder.last().unwrap(), "fbp federal bureau of physics");
    // colon-split + alias rungs present.
    assert!(ladder.contains(&"federal bureau of physics 001".to_string()));
    assert!(ladder.contains(&"collider 001".to_string()));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p longbox-newznab -- query::tests::search_ladder`
Expected: FAIL (`search_ladder` undefined).

- [ ] **Step 3: Implement `search_ladder` (Step 1 code) and run**

Add the `search_ladder` fn from Step 1 to `query.rs`. Run the test:
Run: `cargo test -p longbox-newznab -- query::tests::search_ladder`
Expected: PASS.

- [ ] **Step 4: Rewrite `search_one_indexer` to walk the ladder**

In `longbox-newznab/src/client.rs`, replace the body of `search_one_indexer` (`client.rs:126-160`) — add `aliases: &[String]` to its signature and loop the ladder, stopping on first non-empty:

```rust
async fn search_one_indexer(
    client: &reqwest::Client,
    indexer: &IndexerConfig,
    series: &str,
    issue: &str,
    aliases: &[String],
    maxage_override: Option<i64>,
) -> Result<Vec<Release>, IndexerError> {
    let ladder = crate::query::search_ladder(series, issue, aliases);
    let mut last_err: Option<IndexerError> = None;
    for (i, term) in ladder.iter().enumerate() {
        match search_with(client, indexer, term, maxage_override).await {
            Ok(results) if !results.is_empty() => return Ok(results),
            Ok(_) => {}
            Err(e) => {
                // First rung's failure is the indexer's failure — propagate so
                // the caller can attribute it. Later rungs are best-effort
                // (a clean earlier empty already proved reachability).
                if i == 0 {
                    return Err(e);
                }
                tracing::debug!(
                    target: "longbox_newznab",
                    indexer = %indexer.name,
                    term = %term,
                    error = %e,
                    "ladder rung failed; continuing"
                );
                last_err = Some(e);
            }
        }
    }
    match last_err {
        Some(e) if ladder.len() == 1 => Err(e),
        _ => Ok(Vec::new()),
    }
}
```

- [ ] **Step 5: Update both `search_one_indexer` call sites**

In `find_release_excluding` (`client.rs:226`) and `find_release_excluding_filtered` (`client.rs:330`), add the `aliases` argument. The bare variant has no aliases — pass `&[]`:

```rust
        // client.rs:226 (bare variant)
        match search_one_indexer(&client, indexer, series, issue, &[], Some(effective_maxage)).await {
```
```rust
        // client.rs:330 (filtered variant) — `aliases` is now a param (Task 4)
        search_one_indexer(&client, idx, series, issue, aliases, Some(effective_maxage))
```

- [ ] **Step 6: Run the crate tests**

Run: `cargo test -p longbox-newznab`
Expected: PASS. Fix any remaining arity mismatches in `client.rs` tests by passing `&[]` for aliases.

- [ ] **Step 7: Commit**

```bash
git add longbox-newznab/src/client.rs longbox-newznab/src/query.rs
git commit -m "feat(search): relaxation ladder (full -> colon-split -> alias -> title-only)"
```

---

## Commit 5 — CV aliases ingestion + DB column

### Task 7: Migration — `series.aliases` column

**Files:**
- Create: `longbox-db/migrations/20260630120000_add_series_aliases.sql`

- [ ] **Step 1: Write the migration**

```sql
-- CV volume `aliases` (newline-separated alternate titles, e.g. FBP's
-- original "Collider"). Stored verbatim as CV returns it; the pull-search
-- path splits on newline at read time. NULL = none known.
ALTER TABLE series ADD COLUMN aliases TEXT;
```

- [ ] **Step 2: Apply + regenerate sqlx metadata**

```bash
export DATABASE_URL="sqlite:/tmp/lb-prepare.db?mode=rwc"
cargo sqlx database drop -y 2>/dev/null; cargo sqlx database create
cargo sqlx migrate run --source longbox-db/migrations
```
Expected: migration applies cleanly.

- [ ] **Step 3: Commit**

```bash
git add longbox-db/migrations/20260630120000_add_series_aliases.sql
git commit -m "feat(db): add series.aliases column"
```

### Task 8: Capture CV `aliases` through the DTO + projection

**Files:**
- Modify: `longbox-comicvine/src/models.rs` (`CvVolumeFull`)
- Modify: `longbox-comicvine/src/projection.rs` (`CvVolumeDetail`, `project_volume`)

- [ ] **Step 1: Write the failing test**

Add to `longbox-comicvine/src/projection.rs` tests (or create a `#[cfg(test)] mod tests` if none) — a projection test that feeds a `CvVolumeFull` with aliases through `project_volume`. Since `CvVolumeFull` is `pub(crate)`, put this test in `projection.rs`:

```rust
#[cfg(test)]
mod alias_tests {
    use super::*;
    use crate::models::CvVolumeFull;

    #[test]
    fn project_volume_carries_aliases() {
        let raw = CvVolumeFull {
            id: 42,
            name: "FBP: Federal Bureau of Physics".into(),
            start_year: Some("2013".into()),
            publisher: None,
            description: None,
            image: None,
            site_detail_url: "https://x".into(),
            aliases: Some("Collider\nCollider Comics".into()),
        };
        let detail = project_volume(raw);
        assert_eq!(detail.aliases.as_deref(), Some("Collider\nCollider Comics"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p longbox-comicvine project_volume_carries_aliases`
Expected: FAIL to compile (`CvVolumeFull`/`CvVolumeDetail` have no `aliases`).

- [ ] **Step 3: Add the field on both types + map it**

In `longbox-comicvine/src/models.rs`, add to `CvVolumeFull` (after `image`, `models.rs:76`):

```rust
    #[serde(default)]
    pub aliases: Option<String>,
```

In `longbox-comicvine/src/projection.rs`, add to `CvVolumeDetail` (after `site_detail_url`, `projection.rs:30`):

```rust
    /// CV's newline-separated alternate titles (e.g. "Collider" for FBP).
    pub aliases: Option<String>,
```

In `project_volume` (`projection.rs:74-83`), add the mapping:

```rust
        site_detail_url: item.site_detail_url,
        aliases: item.aliases,
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p longbox-comicvine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add longbox-comicvine/src/models.rs longbox-comicvine/src/projection.rs
git commit -m "feat(comicvine): capture volume aliases in DTO + projection"
```

### Task 9: Persist aliases; add `get_aliases` reader

**Files:**
- Modify: `longbox-db/src/series_repo.rs` (`update_series_volume_detail` writes aliases; new `get_aliases`)
- Modify: `longbox-cv-enrichment/src/worker.rs` (pass aliases at `worker.rs:721`)

- [ ] **Step 1: Write the failing test**

`longbox-db` tests are INTEGRATION tests in `longbox-db/tests/*.rs` (not an in-`src` `#[cfg(test)]` module). Add these to the EXISTING `longbox-db/tests/series.rs`, which already has `mod common;`, `use common::{fresh_pool, ...}`, `use longbox_db::{series_repo, NewSeries, ...}`, and a `walking_dead() -> NewSeries` fixture. Use `fresh_pool().await` (in-memory pool with migrations applied) and `series_repo::insert(&pool, NewSeries{..})` to create the row — this is the real pattern; there is no `#[sqlx::test]` or `insert_minimal_series` here.

```rust
#[tokio::test]
async fn aliases_round_trip() {
    let pool = fresh_pool().await;
    let id = series_repo::insert(&pool, walking_dead()).await.unwrap().id;
    series_repo::update_series_volume_detail(
        &pool, id, None, None, None, Some("Collider\nCollider Comics"),
    )
    .await
    .unwrap();
    let aliases = series_repo::get_aliases(&pool, id).await.unwrap();
    assert_eq!(aliases, vec!["Collider".to_string(), "Collider Comics".to_string()]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SQLX_OFFLINE=true cargo test -p longbox-db aliases_round_trip`
Expected: FAIL to compile (`get_aliases` undefined; `update_series_volume_detail` takes 5 args).

- [ ] **Step 3: Add the `aliases` param to the writer**

In `longbox-db/src/series_repo.rs`, change `update_series_volume_detail` (`series_repo.rs:308-331`) to accept and write aliases:

```rust
pub async fn update_series_volume_detail<'e, E>(
    executor: E,
    series_id: i64,
    publisher: Option<&str>,
    description: Option<&str>,
    cover_url: Option<&str>,
    aliases: Option<&str>,
) -> Result<u64>
where
    E: SqliteExecutor<'e>,
{
    let result = sqlx::query!(
        r#"UPDATE series
           SET publisher = ?, description = ?, cover_url = ?, aliases = ?,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = ?"#,
        publisher,
        description,
        cover_url,
        aliases,
        series_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
```

- [ ] **Step 4: Add the `get_aliases` reader**

Add below `update_series_volume_detail`:

```rust
/// Minimum alias length (chars) we'll feed the search/match path. CV aliases
/// are arbitrary user data; a 1-2 char or empty alias is junk that, once it
/// reaches the alias-token strip in `filter_by_series_title`, could erode a
/// legitimate token and manufacture a false match. Drop them at the source —
/// the match-layer issue/year gates only narrow that window, they don't close
/// it. A real generic-but-longer alias colliding remains theoretically
/// possible and is bounded by those gates; widen this floor only if a real
/// false positive surfaces (deterministic-list philosophy: extend on evidence).
const MIN_ALIAS_LEN: usize = 3;

/// Read a series' aliases as a split, trimmed, sanitized list. CV stores them
/// newline-separated; `None`/empty column → empty vec. Entries shorter than
/// `MIN_ALIAS_LEN` are dropped (junk guard, see above). Used only by the pull
/// search path (kept off `SeriesRow` to avoid widening every series SELECT).
pub async fn get_aliases<'e, E>(executor: E, series_id: i64) -> Result<Vec<String>>
where
    E: SqliteExecutor<'e>,
{
    let row = sqlx::query!(r#"SELECT aliases FROM series WHERE id = ?"#, series_id)
        .fetch_optional(executor)
        .await?;
    let raw = row.and_then(|r| r.aliases).unwrap_or_default();
    Ok(raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| l.chars().count() >= MIN_ALIAS_LEN)
        .collect())
}
```

Add this junk-guard test to `longbox-db/tests/series.rs` alongside `aliases_round_trip`:

```rust
#[tokio::test]
async fn get_aliases_drops_short_junk_entries() {
    let pool = fresh_pool().await;
    let id = series_repo::insert(&pool, walking_dead()).await.unwrap().id;
    // Mix of a real alias, an empty line, and 1-2 char junk.
    series_repo::update_series_volume_detail(
        &pool, id, None, None, None, Some("Collider\n\nA\nXY\nReal Name"),
    )
    .await
    .unwrap();
    let aliases = series_repo::get_aliases(&pool, id).await.unwrap();
    assert_eq!(aliases, vec!["Collider".to_string(), "Real Name".to_string()]);
}
```

- [ ] **Step 5: Update the worker call site**

In `longbox-cv-enrichment/src/worker.rs`, the `update_series_volume_detail(...)` call (`worker.rs:721-728`) gains the aliases argument from the projected `volume`:

```rust
    series_repo::update_series_volume_detail(
        &mut *tx,
        series_id,
        volume.publisher.as_deref(),
        volume.description.as_deref(),
        volume.cover_url.as_deref(),
        volume.aliases.as_deref(),
    )
    .await?;
```

- [ ] **Step 6: Regenerate sqlx metadata + run**

```bash
export DATABASE_URL="sqlite:/tmp/lb-prepare.db?mode=rwc"
cargo sqlx prepare --workspace
cargo test -p longbox-db aliases_round_trip
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add longbox-db/src/series_repo.rs longbox-cv-enrichment/src/worker.rs .sqlx
git commit -m "feat(db): persist + read series aliases"
```

### Task 10: Engine fetches aliases + threads into search

**Files:**
- Modify: `longbox-pull/src/engine.rs` (`attempt_pull_for_candidate`: fetch aliases, pass into `find_release_excluding_filtered` at `engine.rs:507`)

- [ ] **Step 1: Fetch aliases before the search**

In `longbox-pull/src/engine.rs`, inside `attempt_pull_for_candidate`, just before the `match longbox_newznab::find_release_excluding_filtered(` call (`engine.rs:507`), add:

```rust
    let aliases = series_repo::get_aliases(db, series.id).await?;
```

- [ ] **Step 2: Pass it into the call**

Extend the `find_release_excluding_filtered(...)` argument list (`engine.rs:507-518`) to append `&aliases` after `min_size_bytes`:

```rust
        issue.cover_date.as_deref(),
        min_size_bytes,
        &aliases,
    )
```

- [ ] **Step 3: Build + test the whole workspace**

Run: `cargo build --workspace && cargo test --workspace`
Expected: PASS. Fix any remaining `find_release_excluding_filtered` callers (e.g. `sweep_single_issue` path, other engine call sites — grep `find_release_excluding_filtered` in `longbox-pull`/`longbox-web`) by fetching + passing aliases the same way, or `&[]` where a series id isn't in scope.

- [ ] **Step 4: Commit**

```bash
git add longbox-pull/src/engine.rs
git commit -m "feat(pull): thread series aliases into indexer search"
```

---

## Final verification

- [ ] **Full workspace green:**
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Live smoke test (FBP):** rebuild the container, trigger a single-issue pull-search for "FBP: Federal Bureau of Physics" #1, confirm a non-empty result where it previously returned zero. (Uses the Colima/Docker rebuild loop — see `longbox_deployment_notes`.)

- [ ] **Regression spot-check:** pick 3 colon titles from the catalog (`BRZRKR: The Bleeding Tide`, `Black Hammer: Age of Doom`, `Bone Orchard: Tenement`) and confirm search still resolves a correct issue (right series, right issue number).

---

## Out of scope (flagged, do not build here)

- **Generic drop-leading-token rung** → GH #12 (only 1 true acronym-name case in the catalog; rest are publisher prefixes handled by Task 1–2).
- **Backfill** of the existing missing-issue work-list against the fixed search path → separate follow-up after this ships.
- **UI surfacing of aliases** → cheap fast-follow, not required this pass.

## Self-review notes

- Spec coverage: Problem 1 → Tasks 5,6 (+2 hyphen agreement); Problem 2 → Tasks 4,7,8,9,10; Problem 3 → Tasks 1,2. Issue-gate prerequisite → Task 3. All four locked decisions implemented.
- Type consistency: `filter_by_series_title` final signature = `(releases, patterns, requested_series_title, requested_year, threshold, exclusion_keywords, min_size_bytes, requested_issue, aliases)` — Tasks 3 and 4 append in that order; the `client.rs` call and all tests must match. `find_release_excluding_filtered` gains `aliases` (Task 4) and its inner `search_one_indexer` gains `aliases` (Task 6). `update_series_volume_detail` gains trailing `aliases` (Task 9).
- Known limitation (intentional): the ladder stops on first RAW-non-empty rung; a rung that returns junk-but-non-empty won't fall through to title-only. Acceptable — the reported bug is zero-results, which the ladder resolves. Revisit only if post-filter-empty misses surface.
