# Item A v2 piece 4 / Option C — live probe + close-out

Date: 2026-05-31
Commits: 9ceab4b (feature), e2b176f (regression tests)

## What shipped

`POST /api/releases/calendar/pull` accepts either `cv_volume_id` (CV path,
unchanged) **or** `metron_series_id` (new). The Metron path resolves
cv_id via Metron's `fetch_series_detail`, then funnels into the existing
`add_or_get_from_cv` flow, then lazy-backfills `series.metron_id` so the
next call short-circuits via the catalog.

Bulk endpoint `POST /api/releases/calendar/pull/bulk` body shape changed
from `{cv_volume_ids: number[]}` to `{items: SubscribeTarget[]}` where each
item is either `{cv_volume_id}` or `{metron_series_id}`. Per-item results
echo back whichever id was sent so the frontend can correlate.

Error mapping additions in `longbox-web/src/error.rs`:
- New `ApiError::ServiceUnavailable { code, message }` → 503, used when
  `state.metron` is None and the request can only be resolved via Metron.
- `From<MetronError> for ApiError`: `NotFound` → 404
  `not_found.metron_resource`; `Timeout` / `Network` / `Http` → 502
  `upstream.metron`; `RateLimited` → 503 + retry-after.

Frontend selection key changed from `number` to a discriminator string
(`cv:{id}` / `metron:{id}`). This is the root cause / fix for the
checkbox bug Jeremy hit live: piece-3's `sel.has(row.cv_volume_id)`
collapsed to `sel.has(null)` on every Metron forward-week row, so one
click visually checked every box. Piece-4 keys each row by either its
cv_volume_id or its metron_series_id, never null. Two regression tests
in e2b176f codify the invariant.

## Live probe — all four code paths exercised against prod Metron

Container: `docker-compose up -d --build --force-recreate`, healthy
in 1s. Metron credentials in `.env` as `METRON_API_USER` /
`METRON_API_PASSWORD`; `metron_enabled = true` in settings.

**1. Subscribe via `metron_series_id=10959` (Absolute Green Lantern):**
`{"series_id": 54}` returned. Round-trip < 1s. Metron resolved
cv_id=163145, `add_or_get_from_cv` created series id=54,
`backfill_metron_id` wrote `series.metron_id = '10959'`.

**2. SQL verify backfill:** `54 | 163145 | 10959 | Absolute Green Lantern | DC Comics`.
metron_id populated. publisher populated via the Metron series-detail
payload feeding the CV-create path.

**3. Second subscribe, same `metron_series_id=10959`:** `{"series_id": 54}`
returned (same row). Catalog-first path firing — `series.metron_id` JOIN
resolved directly, no Metron round-trip. This is the load-bearing
catalog-cache assertion from the api_test.

**4. Negative — bogus `metron_series_id=999999`:** 404 with
`code: not_found.metron_resource`. Metron's 404 propagated cleanly
through `MetronError::NotFound` → `ApiError::NotFound`. Not 500.

The two paths the live probe couldn't exercise (Metron returns cv_id
None → 422 fallback; `state.metron = None` → 503) are both covered by
the four api_tests in `longbox-web/tests/api_tests.rs`. Together: every
documented Option C branch is covered by either a test or a live probe.

## Probe artifact cleanup

The Absolute Green Lantern subscription created during the probe was
removed via the WAL-rule sidecar pattern (stop → sidecar
`DELETE FROM issues WHERE series_id=54; DELETE FROM series WHERE id=54` →
start). 14 issue rows + 1 series row deleted. `pull_list` row was
previously removed via `DELETE /api/pull-list/54`. `cv_volume_cache` /
`metron_calendar_cache` entries left in place — they're caches keyed
independently and harmless.

## What's still open

Nothing for Option C itself. The Item A v2 arc (pieces 1-4) is complete.

Outstanding for the broader feature area:
- Calendar `metron_calendar_cache` has a 24h TTL; first cold-cache fetch
  of a forward week still takes ~3 min wall time (rate-limiter-bound on
  the per-issue hydration). Documented as operational, not a defect.
- `gcd_*` settings rows shipped as placeholders in piece 2 but no GCD
  integration exists. Settings rows are dead-code-flagged.

## Tests passing as of close-out

- `cargo test --workspace`: full sweep green.
- Frontend: 29 files / 207 tests passing (12 calendar page tests, including
  the two new isolation regressions).
- Cargo clippy: clean except one pre-existing range-contains warning in
  longbox-comicvine unrelated to this arc.
