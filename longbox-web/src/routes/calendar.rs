//! Release calendar — `GET /api/releases/calendar` (cache-aware CV
//! release-date query) and `POST /api/releases/calendar/pull` (the
//! compound "add this volume to the pull list" action).
//!
//! The CV payload is cached in `cv_release_cache` keyed by the date
//! range (the `publisher` key is always empty in v1 — CV's `/issues/`
//! query carries no publisher, so there is no publisher column or
//! filter). Pull-list / tracked-series state is *not* cached: it changes
//! independently of the CV data, so each response enriches the cached
//! rows live.

use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_comicvine::CvCalendarItem;
use longbox_core::normalize_title;
use futures::stream::{self, StreamExt};
use longbox_db::{
    cv_volume_cache_repo, metron_calendar_cache_repo, pull_list_repo, release_cache_repo,
    series_repo, settings_repo, NewPullEntry, NewReleaseCacheEntry,
};
use longbox_metron::{MetronCalendarItem, MetronClient};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::error::ApiError;
use crate::routes::series::{add_or_get_from_cv, spawn_auto_rematch};
use crate::state::{AppState, SubscribeOutcome};

/// How long a cached CV release-calendar payload stays fresh. A user
/// query inside this window is served from `cv_release_cache`; past it
/// (or with `?refresh=true`) the handler re-queries ComicVine.
const CALENDAR_CACHE_TTL: time::Duration = time::Duration::hours(1);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/releases/calendar", get(calendar))
        .route("/releases/calendar/pull", post(add_to_pull_list))
        .route("/releases/calendar/pull/bulk", post(add_to_pull_list_bulk))
        .route("/releases/calendar/pull/status", get(subscribe_status))
        .route("/releases/of-note", get(releases_of_note))
        .route("/releases/this-weeks-pulls", get(this_weeks_pulls))
}

// -------- shapes --------

#[derive(Debug, Deserialize)]
struct CalendarQuery {
    from: String,
    to: String,
    /// Force a CV re-query, bypassing a fresh cache entry — the
    /// "Refresh CV" button.
    #[serde(default)]
    refresh: bool,
}

/// Unified calendar response row. Sourced from CV for current/past
/// weeks and Metron for forward weeks; the two paths produce this same
/// shape so the frontend doesn't branch on source.
///
/// Identity columns (`cv_issue_id`, `cv_volume_id`, `metron_issue_id`)
/// are all `Option<i64>`. At least one of `cv_issue_id` or
/// `metron_issue_id` is always `Some` — the frontend uses
/// `cv_issue_id ?? metron_issue_id` as the keyed-each key. CV-sourced
/// rows have `cv_issue_id` + `cv_volume_id` populated and
/// `metron_issue_id = None`. Metron-sourced rows always have
/// `metron_issue_id`; `cv_issue_id` and `cv_volume_id` are populated
/// only when Metron supplies them inline (i.e., CV has already
/// catalogued the issue / volume).
///
/// `series_id` is set when the issue's volume maps to a tracked series.
/// `on_pull_list` when that series is subscribed. `publisher` chain:
///   1. tracked series publisher (CV path),
///   2. cv_volume_cache (CV path),
///   3. Metron-inline publisher (Metron path),
///   4. None — frontend groups as "Unknown Publisher".
#[derive(Debug, Serialize)]
struct CalendarRow {
    cv_issue_id: Option<i64>,
    cv_volume_id: Option<i64>,
    metron_issue_id: Option<i64>,
    /// Metron's series id. Populated on Metron-sourced rows; the
    /// Option C subscription path sends it back so the handler can
    /// resolve a cv_volume_id (via catalog-first lookup, falling
    /// through to `fetch_series_detail`) before continuing the CV-side
    /// flow.
    metron_series_id: Option<i64>,
    issue_number: String,
    store_date: String,
    volume_name: String,
    cover_url: Option<String>,
    site_detail_url: String,
    series_id: Option<i64>,
    on_pull_list: bool,
    publisher: Option<String>,
}

/// Add-to-pull-list request. Item A v2 Option C extension: accepts
/// either `cv_volume_id` (CV-sourced calendar rows, existing flow) OR
/// `metron_series_id` (Metron-sourced forward-week rows, resolved to a
/// cv_volume_id via the Metron series-detail endpoint at subscription
/// time). Exactly one must be present.
#[derive(Debug, Deserialize)]
struct AddToPullBody {
    #[serde(default)]
    cv_volume_id: Option<i64>,
    #[serde(default)]
    metron_series_id: Option<i64>,
}

/// Bulk-add request — same one-of-two-fields per item as the single
/// add.
#[derive(Debug, Deserialize)]
struct BulkAddBody {
    items: Vec<AddToPullBody>,
}

/// Per-item outcome of a bulk add. `status` is one of `added`,
/// `already_on_list`, or `failed`. `cv_volume_id` is the cv_id the
/// subscription resolved to (the request value for CV-sourced items,
/// the resolved value for Metron-sourced items); `None` when
/// resolution itself failed (e.g. Metron series with no cv_id).
/// `metron_series_id` echoes the request's value when the item used
/// the Metron path, so the frontend can correlate results back to
/// request items without relying on order.
#[derive(Debug, Serialize)]
struct BulkAddResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    cv_volume_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metron_series_id: Option<i64>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    series_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BulkAddResponse {
    results: Vec<BulkAddResult>,
}

/// One "release of note" — a volume from this ship-week's calendar whose
/// name matches a series the user owns and that is not on the pull list.
/// Deduped to one row per volume.
#[derive(Debug, Serialize)]
struct ReleaseOfNote {
    cv_volume_id: i64,
    volume_name: String,
    cover_url: Option<String>,
    site_detail_url: String,
    /// How many of the volume's issues land in the ship-week.
    issue_count: i64,
}

// -------- handlers --------

/// The release calendar for a date range. Two source paths, dispatched
/// on whether the window is in the past/current or in the future:
///
/// - `from <= today_utc()` → ComicVine. Cache-aware: a fresh
///   `cv_release_cache` row is served as-is; otherwise CV is queried
///   and the result re-cached. Rows are enriched live with pull-list
///   state before returning.
///
/// - `from > today_utc()` AND `state.metron.is_some()` → Metron forward
///   path (Item A v2). Cache-aware against `metron_calendar_cache`;
///   fully-hydrated payloads (publisher already resolved) returned
///   directly.
///
/// - `from > today_utc()` AND `state.metron.is_none()` → empty result.
///   Forward weeks fall through to an empty response when Metron is
///   disabled or credentials are bad; no error surfaces to the user.
///
/// The two paths never appear in one response — the calendar's 7-day
/// navigation window is always entirely past/current OR entirely
/// future. UTC throughout: `today` here is `OffsetDateTime::now_utc()`,
/// matching `current_ship_week()` below. Timezone edges don't show up
/// at the week boundary.
async fn calendar(
    State(state): State<AppState>,
    Query(q): Query<CalendarQuery>,
) -> Result<Json<Vec<CalendarRow>>, ApiError> {
    if !is_date_shaped(&q.from) || !is_date_shaped(&q.to) {
        return Err(ApiError::BadRequest {
            message: "`from` and `to` must be YYYY-MM-DD dates".into(),
        });
    }

    // UTC today, matching current_ship_week() below.
    let today_utc = OffsetDateTime::now_utc().date();
    let from_date =
        time::Date::parse(&q.from, &time::format_description::well_known::Iso8601::DATE)
            .map_err(|_| ApiError::BadRequest {
                message: "`from` must be a valid YYYY-MM-DD date".into(),
            })?;

    if from_date > today_utc {
        // Forward-week path (Item A v2).
        match &state.metron {
            Some(client) => {
                let rows = load_metron_calendar(&state, client, &q.from, &q.to).await?;
                Ok(Json(rows))
            }
            None => Ok(Json(Vec::new())),
        }
    } else {
        // CV path (existing, unchanged behavior).
        let items = load_cv_calendar(&state, &q.from, &q.to, q.refresh).await?;
        Ok(Json(enrich(&state, items).await?))
    }
}

/// Resolve one CV volume to a tracked series (creating it from
/// ComicVine when LongBox doesn't track it yet — the Library Tidy
/// `add_or_get_from_cv` primitive) and subscribe it to the pull list.
/// Returns the series id and whether the subscription was newly
/// `added` or the series was `already_on_list`. Both halves are
/// idempotent; shared by the single and bulk add handlers.
async fn try_add_one(state: &AppState, cv_volume_id: i64) -> Result<(i64, &'static str), ApiError> {
    if cv_volume_id <= 0 {
        return Err(ApiError::BadRequest {
            message: "cv_volume_id must be > 0".into(),
        });
    }
    let (series, was_new) = add_or_get_from_cv(state, cv_volume_id).await?;
    if was_new {
        // A freshly created series: match any of its files already on
        // disk, same as the /files and Library Tidy add paths.
        spawn_auto_rematch(state, series.id, "calendar-add");
    }
    let status = if pull_list_repo::get(&state.db, series.id).await?.is_none() {
        pull_list_repo::add(
            &state.db,
            NewPullEntry {
                series_id: series.id,
                start_issue: None,
            },
        )
        .await?;
        // Auto-fire an on-demand search for the freshly-subscribed
        // series. Bulk-add fires N of these (one per item); duplicate
        // series_ids in the payload are absorbed by the per-series
        // guard. No toast — the user sees results land naturally.
        // Only fires on a real "added" transition; an already-on-list
        // no-op doesn't re-search.
        state.pull_search.try_start(series.id).await;
        "added"
    } else {
        "already_on_list"
    };
    Ok((series.id, status))
}

/// Compound "add to pull list": resolve the request to a cv_volume_id
/// (CV path direct, Metron path via [`resolve_cv_id_via_metron`]), then
/// subscribe via the shared [`try_add_one`]. Idempotent — a volume
/// already subscribed is a clean no-op `200`. Single-item add is
/// synchronous (one item never stampedes the CV burst); the bulk path
/// is the one that defers not-yet-local items to the background.
async fn add_to_pull_list(
    State(state): State<AppState>,
    Json(body): Json<AddToPullBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (series_id, _status) = resolve_and_subscribe(&state, &body).await?;
    Ok(Json(serde_json::json!({ "series_id": series_id })))
}

/// Resolve a request to a cv_volume_id and subscribe it, firing the
/// best-effort `metron_id` backfill on the Metron path. The shared core
/// of the single add and of each background resolution in the bulk path.
/// May perform a ComicVine fetch when the volume isn't local yet — so
/// callers that must stay fast (the bulk handler) run it off the request
/// thread, behind [`AppState::cv_resolve_gate`].
async fn resolve_and_subscribe(
    state: &AppState,
    body: &AddToPullBody,
) -> Result<(i64, &'static str), ApiError> {
    let (cv_volume_id, metron_origin) = resolve_subscription_target(state, body).await?;
    let (series_id, status) = try_add_one(state, cv_volume_id).await?;
    if let Some(metron_series_id) = metron_origin {
        backfill_metron_id(state, series_id, metron_series_id).await;
    }
    Ok((series_id, status))
}

/// Bulk "add to pull list". Splits the batch by cost so the request
/// returns fast and never fails silently:
///
/// - **Already-local volumes** (a `cv_volume_id` that maps to a tracked
///   series) subscribe *synchronously* — no ComicVine touch, sub-second
///   for the common case. Result status is the real `added` /
///   `already_on_list`.
/// - **Not-yet-local volumes and all Metron-path items** need a network
///   resolve (CV `fetch_volume` + `fetch_issues`, and/or a Metron
///   series-detail call). They come back `resolving` and are finished by
///   a background resolver; the frontend polls
///   `GET /releases/calendar/pull/status` for the terminal outcome.
/// - **Malformed items** (`cv_volume_id <= 0`, or neither id present)
///   fail synchronously with a reason.
///
/// Why the split: the prior version ran every item through
/// `buffer_unordered(10)`, firing up to 10 concurrent CV fetches into a
/// 5-token-burst / 1-per-20s / 60s-max-wait limiter. The burst emptied
/// instantly; the rest waited 20-60s (the stall) or exceeded 60s and
/// returned `CvError::RateLimited` as a silent `failed`. A brand-new
/// series needs two tokens, doubling the worst case to ~2 minutes. The
/// concurrency WAS the bug. The background resolver drains items
/// *sequentially* through [`AppState::cv_resolve_gate`] (one permit),
/// which never trips `max_wait`, killing both the silent failures and the
/// self-perpetuating retry (a RateLimited item is never cached, so it
/// re-fetches forever).
async fn add_to_pull_list_bulk(
    State(state): State<AppState>,
    Json(body): Json<BulkAddBody>,
) -> Result<Json<BulkAddResponse>, ApiError> {
    // Drop terminal leftovers from earlier batches so the in-memory status
    // map stays bounded (read-once pruning in the status endpoint handles
    // the rest under normal polling).
    state
        .subscribe_status
        .write()
        .await
        .items
        .retain(|_, o| !o.is_terminal());

    let mut results = Vec::with_capacity(body.items.len());
    for item in body.items {
        // Synchronous fast path: a cv_volume_id already tracked locally
        // subscribes with no network call. Also catches malformed input.
        if let Some(cv) = item.cv_volume_id {
            if cv <= 0 {
                results.push(BulkAddResult {
                    cv_volume_id: Some(cv),
                    metron_series_id: None,
                    status: "failed",
                    series_id: None,
                    error: Some("cv_volume_id must be > 0".into()),
                });
                continue;
            }
            if series_repo::find_by_cv_id(&state.db, cv).await?.is_some() {
                // Local: try_add_one re-reads find_by_cv_id (cheap) and
                // subscribes; no CV fetch happens.
                results.push(match try_add_one(&state, cv).await {
                    Ok((series_id, status)) => BulkAddResult {
                        cv_volume_id: Some(cv),
                        metron_series_id: None,
                        status,
                        series_id: Some(series_id),
                        error: None,
                    },
                    Err(e) => BulkAddResult {
                        cv_volume_id: Some(cv),
                        metron_series_id: None,
                        status: "failed",
                        series_id: None,
                        error: Some(e.to_string()),
                    },
                });
                continue;
            }
        } else if item.metron_series_id.is_none() {
            results.push(BulkAddResult {
                cv_volume_id: None,
                metron_series_id: None,
                status: "failed",
                series_id: None,
                error: Some(
                    "exactly one of `cv_volume_id` or `metron_series_id` is required".into(),
                ),
            });
            continue;
        }

        // Background path: not-yet-local CV volume, or any Metron item.
        let key = item_key(&item);
        let cv_echo = item.cv_volume_id;
        let metron_echo = item.metron_series_id;
        mark_resolving(&state, &key).await;
        spawn_background_resolve(state.clone(), item, key);
        results.push(BulkAddResult {
            cv_volume_id: cv_echo,
            metron_series_id: metron_echo,
            status: "resolving",
            series_id: None,
            error: None,
        });
    }

    Ok(Json(BulkAddResponse { results }))
}

/// The frontend row discriminator for an add request — `cv:{id}` or
/// `metron:{id}`, matching the key the calendar UI builds, so a
/// background outcome correlates back to the right row. Only called on
/// the background path, where at least one id is present.
fn item_key(item: &AddToPullBody) -> String {
    match (item.cv_volume_id, item.metron_series_id) {
        (Some(cv), _) => format!("cv:{cv}"),
        (None, Some(m)) => format!("metron:{m}"),
        // Unreachable on the background path (malformed items fail
        // synchronously upstream); a stable fallback beats a panic.
        (None, None) => "invalid".to_string(),
    }
}

/// Mark a key `resolving` before its background task starts, so a status
/// poll that races ahead of the task still sees the in-flight state.
async fn mark_resolving(state: &AppState, key: &str) {
    state.subscribe_status.write().await.items.insert(
        key.to_string(),
        SubscribeOutcome {
            status: "resolving".to_string(),
            series_id: None,
            error: None,
        },
    );
}

/// Resolve + subscribe one deferred item off the request thread, behind
/// the [`AppState::cv_resolve_gate`] so CV fetches stay strictly
/// sequential. The terminal outcome is written to `subscribe_status`
/// under `key`; a failure is logged at warn (server-side surface) AND
/// recorded with its reason (UI surface), never silently dropped.
fn spawn_background_resolve(state: AppState, item: AddToPullBody, key: String) {
    tokio::spawn(async move {
        let _permit = state
            .cv_resolve_gate
            .acquire()
            .await
            .expect("cv_resolve_gate is never closed");
        let outcome = match resolve_and_subscribe(&state, &item).await {
            Ok((series_id, status)) => SubscribeOutcome {
                status: status.to_string(),
                series_id: Some(series_id),
                error: None,
            },
            Err(e) => {
                tracing::warn!(
                    target: "longbox_web",
                    key = %key,
                    error = %e,
                    "pull-list background resolve failed"
                );
                SubscribeOutcome {
                    status: "failed".to_string(),
                    series_id: None,
                    error: Some(e.to_string()),
                }
            }
        };
        state
            .subscribe_status
            .write()
            .await
            .items
            .insert(key, outcome);
    });
}

/// Poll endpoint for background pull-list resolutions. Returns the
/// current `{ items: { key: {status, series_id, error} } }` snapshot and
/// prunes the terminal entries it returns (read-once) — bounded growth
/// for a single-user app. The frontend polls until its `resolving` keys
/// have all gone terminal.
async fn subscribe_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut store = state.subscribe_status.write().await;
    let snapshot = serde_json::json!({ "items": store.items });
    store.items.retain(|_, o| !o.is_terminal());
    Json(snapshot)
}

/// Item A v2 Option C: resolve a request's identity to a CV volume id,
/// taking the catalog-first cache path before any Metron API call.
/// Returns `(cv_volume_id, metron_origin)` — `metron_origin` is the
/// `metron_series_id` from the request when that path was used, so the
/// caller can fire the lazy backfill after a successful subscription.
async fn resolve_subscription_target(
    state: &AppState,
    body: &AddToPullBody,
) -> Result<(i64, Option<i64>), ApiError> {
    match (body.cv_volume_id, body.metron_series_id) {
        // Both present: cv_volume_id wins (faster path, no Metron round trip).
        (Some(cv), _) => Ok((cv, None)),
        (None, Some(metron_series_id)) => {
            let cv = resolve_cv_id_via_metron(state, metron_series_id).await?;
            Ok((cv, Some(metron_series_id)))
        }
        (None, None) => Err(ApiError::BadRequest {
            message: "exactly one of `cv_volume_id` or `metron_series_id` is required".into(),
        }),
    }
}

/// Item A v2 Option C — resolve a Metron series id to a cv_volume_id
/// via the two-tier chain:
///
///  1. **Catalog-first cache.** Look up `series.metron_id` in the
///     local catalog. If the series exists AND has `cv_id` populated,
///     return that cv_id with zero API calls. This is the optimization
///     that makes second-and-later subscriptions for the same series
///     free of Metron round-trips.
///  2. **Metron series-detail fallback.** Catalog miss (or series
///     exists but cv_id is NULL) → call `client.fetch_series_detail`.
///     If the detail carries a cv_id, return it (the caller fires
///     the lazy backfill after subscription succeeds). If not,
///     return 422 with the user-facing fallback message — coverage
///     is good but not 100%, and the user-actionable message handles
///     the miss case gracefully.
async fn resolve_cv_id_via_metron(
    state: &AppState,
    metron_series_id: i64,
) -> Result<i64, ApiError> {
    // ---- Tier 1: catalog-first cache ----
    let metron_id_str = metron_series_id.to_string();
    if let Some(series) = series_repo::find_by_metron_id(&state.db, &metron_id_str).await? {
        if let Some(cv_id) = series.cv_id {
            return Ok(cv_id);
        }
    }

    // ---- Tier 2: Metron API ----
    let Some(client) = &state.metron else {
        return Err(ApiError::ServiceUnavailable {
            code: "metron_not_configured",
            message: "Metron integration not configured".into(),
        });
    };
    let detail = client.fetch_series_detail(metron_series_id).await?;
    detail.cv_id.ok_or_else(|| ApiError::Unprocessable {
        code: "metron_series_not_in_cv",
        message: "This series isn't yet indexed by ComicVine — try again after the next release cycle.".into(),
    })
}

/// Lazy backfill of `series.metron_id` after a successful Metron-path
/// subscription. **Non-critical** — the subscription has already
/// committed by the time this runs. Failures (concurrent write, series
/// deleted, DB hiccup) log at warn and never propagate to the response.
async fn backfill_metron_id(state: &AppState, series_id: i64, metron_series_id: i64) {
    let metron_id_str = metron_series_id.to_string();
    match series_repo::set_metron_id(&state.db, series_id, &metron_id_str).await {
        Ok(0) => {
            // Zero rows = either a concurrent resolution already wrote
            // (idempotent — fine), or the series_id was deleted out
            // from under us (shouldn't happen but harmless). Neither
            // is an error worth surfacing.
            tracing::warn!(
                target: "longbox_web",
                series_id,
                metron_series_id,
                "metron_id backfill: zero rows affected (concurrent or stale)"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                target: "longbox_web",
                series_id,
                metron_series_id,
                error = %e,
                "metron_id backfill: write failed (non-critical)"
            );
        }
    }
}

/// "Releases of note" — the current ship-week's calendar, narrowed to
/// volumes whose name matches a series the user *owns* and that is *not*
/// already on the pull list. A dashboard discovery affordance, deduped
/// to one row per volume.
///
/// The name match runs here, not in a repo method: the calendar payload
/// is cached CV JSON (`cv_release_cache.payload_json`), not table rows,
/// so there is nothing to `LIKE`-join against. An owned series' (already
/// normalized) `sort_title` must be a substring of the release's
/// `normalize_title`-d `volume_name` — see the brief's known v1 tradeoff
/// on short-title false positives.
async fn releases_of_note(
    State(state): State<AppState>,
) -> Result<Json<Vec<ReleaseOfNote>>, ApiError> {
    let (from, to) = current_ship_week();
    let items = load_cv_calendar(&state, &from, &to, false).await?;

    let series = series_repo::find_all_with_counts(&state.db).await?;
    // Normalized titles of series the user owns at least one file of.
    let owned_titles: Vec<String> = series
        .iter()
        .filter(|s| s.owned_count > 0 && !s.series.sort_title.is_empty())
        .map(|s| s.series.sort_title.clone())
        .collect();
    // cv_id of every series currently on the pull list.
    let id_to_cv: HashMap<i64, i64> = series
        .iter()
        .filter_map(|s| s.series.cv_id.map(|cv| (s.series.id, cv)))
        .collect();
    let pulled_cv_ids: HashSet<i64> = pull_list_repo::list_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|e| id_to_cv.get(&e.series_id).copied())
        .collect();

    // Match + dedup by volume, preserving the calendar's order.
    let mut index: HashMap<i64, usize> = HashMap::new();
    let mut out: Vec<ReleaseOfNote> = Vec::new();
    for item in items {
        if pulled_cv_ids.contains(&item.cv_volume_id) {
            continue;
        }
        let haystack = normalize_title(&item.volume_name);
        if !owned_titles.iter().any(|t| haystack.contains(t.as_str())) {
            continue;
        }
        if let Some(&i) = index.get(&item.cv_volume_id) {
            out[i].issue_count += 1;
        } else {
            index.insert(item.cv_volume_id, out.len());
            out.push(ReleaseOfNote {
                cv_volume_id: item.cv_volume_id,
                volume_name: item.volume_name,
                cover_url: item.cover_url,
                site_detail_url: item.site_detail_url,
                issue_count: 1,
            });
        }
    }
    Ok(Json(out))
}

/// "This week's pulls" — the current ship-week's calendar narrowed to
/// issues whose volume is on the pull list. Per-issue, no dedup: it's a
/// what's-shipping-for-me glance. Calendar-based, so the date is the
/// accurate on-sale `store_date` — see the brief's Step 9b note on the
/// `store_date` model superseding the original `cover_date` framing.
async fn this_weeks_pulls(
    State(state): State<AppState>,
) -> Result<Json<Vec<CvCalendarItem>>, ApiError> {
    let (from, to) = current_ship_week();
    let items = load_cv_calendar(&state, &from, &to, false).await?;
    let pulled = pulled_cv_ids(&state).await?;
    Ok(Json(
        items
            .into_iter()
            .filter(|i| pulled.contains(&i.cv_volume_id))
            .collect(),
    ))
}

// -------- internals --------

/// The `cv_id` of every series currently on the pull list.
async fn pulled_cv_ids(state: &AppState) -> Result<HashSet<i64>, ApiError> {
    let id_to_cv: HashMap<i64, i64> = series_repo::find_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|s| s.cv_id.map(|cv| (s.id, cv)))
        .collect();
    Ok(pull_list_repo::list_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|e| id_to_cv.get(&e.series_id).copied())
        .collect())
}

/// The CV calendar payload for `[from, to]`, from cache when fresh and
/// not force-refreshed, otherwise re-queried from ComicVine and cached.
async fn load_cv_calendar(
    state: &AppState,
    from: &str,
    to: &str,
    refresh: bool,
) -> Result<Vec<CvCalendarItem>, ApiError> {
    if !refresh {
        if let Some(row) = release_cache_repo::get(&state.db, from, to, "").await? {
            if is_fresh(row.cached_at) {
                return serde_json::from_str(&row.payload_json).map_err(|e| ApiError::Internal {
                    message: format!("cached calendar payload is corrupt: {e}"),
                    source: anyhow::anyhow!(e),
                });
            }
        }
    }
    let items = state.cv.fetch_release_calendar(from, to).await?;
    let payload_json = serde_json::to_string(&items).map_err(|e| ApiError::Internal {
        message: format!("serializing calendar payload failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    release_cache_repo::upsert(
        &state.db,
        NewReleaseCacheEntry {
            date_from: from.to_owned(),
            date_to: to.to_owned(),
            publisher: String::new(),
            payload_json,
        },
    )
    .await?;
    Ok(items)
}

/// Enrich CV calendar items with live tracked-series / pull-list state
/// plus the publisher resolution chain.
///
/// **Publisher resolution** (Item E v2):
///   1. Series JOIN — when the volume maps to a tracked series with
///      publisher populated, that wins.
///   2. cv_volume_cache fallback — for volumes NOT in the catalog (or
///      catalog rows whose publisher is still NULL), look up the
///      per-volume cache. Populated by the enrichment worker's
///      cache-fill pass; rows that haven't been filled yet carry NULL
///      publisher in the cache.
///   3. NULL — the frontend groups these under "Unknown Publisher".
///
/// CV's `/issues/` query returns no publisher (see this file's
/// module docs), so the JOIN + cache fallback is the only path.
///
/// Both lookups are HashMaps built from one query each: never a
/// per-item DB call inside the loop. Calendar weeks routinely have
/// 100+ items; per-item queries would mean 100+ round trips per
/// request, which is unacceptable for the calendar's load shape.
///
/// **Queue side effect**: any item whose cv_volume_id is neither in
/// the series JOIN nor in cv_volume_cache gets bulk-INSERT-OR-IGNORE'd
/// as a pending cache row before the function returns. The worker's
/// cache-fill pass drains those on its next iteration. Synchronous
/// rather than fire-and-forget: the insert is trivially fast (~1ms
/// for 99 ids, single transaction) and removes the race where a user
/// reloads before a spawned task completes the queue write.
async fn enrich(
    state: &AppState,
    items: Vec<CvCalendarItem>,
) -> Result<Vec<CalendarRow>, ApiError> {
    // cv_volume_id -> (series_id, publisher), for every series LongBox
    // tracks by cv_id. publisher is None for CV-linked series whose
    // 6c.5 refresh-pass hasn't run yet.
    let by_cv: HashMap<i64, (i64, Option<String>)> = series_repo::find_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|s| s.cv_id.map(|cv| (cv, (s.id, s.publisher))))
        .collect();
    // cv_volume_id -> publisher, for every row in cv_volume_cache.
    // Rows queued but not yet filled carry publisher = None and read
    // through to "Unknown Publisher" client-side.
    let cache_pub: HashMap<i64, Option<String>> = cv_volume_cache_repo::list_all_publishers(&state.db)
        .await?
        .into_iter()
        .map(|e| (e.cv_volume_id, e.publisher))
        .collect();
    let on_list: HashSet<i64> = pull_list_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|e| e.series_id)
        .collect();

    // Pass 1: build the response rows, applying the publisher chain.
    // While we're walking the items, collect the cv_volume_ids that
    // are NEITHER in the series JOIN NOR in the cache map — those are
    // the queue targets for the cache-fill worker pass.
    let mut to_queue: HashSet<i64> = HashSet::new();
    let rows: Vec<CalendarRow> = items
        .into_iter()
        .map(|item| {
            let series_entry = by_cv.get(&item.cv_volume_id);
            let series_id = series_entry.map(|(sid, _)| *sid);
            let series_publisher = series_entry.and_then(|(_, pub_opt)| pub_opt.clone());
            let cache_hit = cache_pub.get(&item.cv_volume_id);
            // Resolution chain: series JOIN publisher, then cache, then None.
            let publisher = series_publisher.or_else(|| cache_hit.and_then(|p| p.clone()));
            // Queue if the volume has no series JOIN at all (i.e., not
            // a tracked series) AND has never been seen by the cache.
            // Catalog-linked series belong to the refresh pass, not the
            // cache pass — don't double-queue them.
            if series_entry.is_none() && cache_hit.is_none() {
                to_queue.insert(item.cv_volume_id);
            }
            let on_pull_list = series_id.is_some_and(|sid| on_list.contains(&sid));
            CalendarRow {
                cv_issue_id: Some(item.cv_issue_id),
                cv_volume_id: Some(item.cv_volume_id),
                metron_issue_id: None,
                metron_series_id: None,
                issue_number: item.issue_number,
                store_date: item.store_date,
                volume_name: item.volume_name,
                cover_url: item.cover_url,
                site_detail_url: item.site_detail_url,
                series_id,
                on_pull_list,
                publisher,
            }
        })
        .collect();

    // Synchronous queue write: bulk INSERT OR IGNORE inside one tx.
    // Trivially fast even at 99-ish ids; never grows past a single
    // week's worth on any one call.
    if !to_queue.is_empty() {
        let ids: Vec<i64> = to_queue.into_iter().collect();
        // Swallow the error after logging — a queue write failure must
        // not break the calendar response. The worker will catch the
        // missed ids on the next calendar load.
        if let Err(e) = cv_volume_cache_repo::bulk_queue_pending(&state.db, &ids).await {
            tracing::warn!(
                target: "longbox_web",
                error = %e,
                count = ids.len(),
                "cv_volume_cache.queue_failed"
            );
        }
    }

    Ok(rows)
}

/// Item A v2: forward-week calendar via Metron.
///
/// Two phases:
///   - Cache lookup against `metron_calendar_cache(date_from, date_to)`.
///     If a row exists and `cached_at` is within
///     `metron_calendar_cache_ttl_hours` (default 24h), deserialize the
///     payload and return.
///   - Cache miss / stale: fetch the issue list, hydrate each issue's
///     detail in parallel (`buffer_unordered(20)` matching Metron's
///     20-burst quota — first 20 fire concurrently, subsequent calls
///     pace at the rate-limiter's sustained refill), serialize, store,
///     return.
///
/// Cold-cache wall time at the rate-limit shape: ~55-60s for a typical
/// 70-issue forward week. Subsequent loads within the TTL window are
/// instant. Per piece-3 sign-off (cold-cache option 1): we accept the
/// first-load cost and don't pre-build async/polling infrastructure.
///
/// Per-issue hydration failures are logged-and-dropped — a partial
/// hydration is better than a failed calendar response. Empty lists
/// also cache to avoid retrying empty CV-side weeks on every page load.
async fn load_metron_calendar(
    state: &AppState,
    client: &MetronClient,
    from: &str,
    to: &str,
) -> Result<Vec<CalendarRow>, ApiError> {
    let ttl_hours: i64 = settings_repo::get_or_default(
        &state.db,
        "metron_calendar_cache_ttl_hours",
        24,
    )
    .await?;

    // Cache lookup — fast path for repeat loads inside the TTL window.
    if let Some(row) = metron_calendar_cache_repo::get(&state.db, from, to).await? {
        if is_metron_cache_fresh(row.cached_at, ttl_hours) {
            let cached: Vec<MetronCalendarItem> =
                serde_json::from_str(&row.payload_json).map_err(|e| ApiError::Internal {
                    message: format!("cached metron payload is corrupt: {e}"),
                    source: anyhow::anyhow!(e),
                })?;
            return project_metron_items(state, cached).await;
        }
    }

    // Cache miss / stale: fetch + hydrate.
    let list = client.fetch_issues_by_store_date_range(from, to).await?;
    let total = list.len();
    let hydration_results: Vec<Result<MetronCalendarItem, longbox_metron::MetronError>> =
        stream::iter(list)
            .map(|item| async move { client.fetch_issue_detail(item.metron_issue_id).await })
            .buffer_unordered(20)
            .collect()
            .await;

    let mut hydrated: Vec<MetronCalendarItem> = Vec::with_capacity(total);
    for result in hydration_results {
        match result {
            Ok(item) => hydrated.push(item),
            Err(e) => {
                tracing::warn!(
                    target: "longbox_web",
                    error = %e,
                    "metron.hydrate_detail_failed"
                );
            }
        }
    }

    // Cache the hydrated payload — opaque JSON, read back by deserialize
    // on next fresh hit. Empty results cache too: prevents retrying weeks
    // where Metron genuinely has nothing.
    let payload_json = serde_json::to_string(&hydrated).map_err(|e| ApiError::Internal {
        message: format!("serializing metron payload failed: {e}"),
        source: anyhow::anyhow!(e),
    })?;
    metron_calendar_cache_repo::upsert(
        &state.db,
        longbox_db::NewMetronCalendarCacheEntry {
            date_from: from.to_owned(),
            date_to: to.to_owned(),
            payload_json,
        },
    )
    .await?;

    project_metron_items(state, hydrated).await
}

/// Project Metron items into the unified `CalendarRow` shape, applying
/// the two-tier identity chain that actually fires for Metron rows:
///
/// 1. **`series.metron_id` JOIN.** If the catalog has a series whose
///    `metron_id` matches this issue's `metron_series_id`, use that
///    series's id + publisher. Piece-4 backfill populates the column
///    for CV-linked series via `?cv_id=` lookups; until then only
///    series-detail refreshes from the worker can fill it.
///
/// 2. **Metron-inline publisher.** Metron's issue-detail response
///    already carries publisher; use it as the fallback.
///
/// (The CV-id chain that the morning kickoff sketched as "tier 1" is
/// not actually reachable: Metron's `cv_id` on the issue is the
/// CV ISSUE id, not the CV VOLUME id, and `cv_volume_cache` /
/// `series.cv_id` are both volume-keyed. The volume cross-reference
/// would require an extra `fetch_volume` per Metron series; for now
/// we let piece-4's `metron_id` backfill do the cross-reference work
/// on a per-series basis.)
async fn project_metron_items(
    state: &AppState,
    items: Vec<MetronCalendarItem>,
) -> Result<Vec<CalendarRow>, ApiError> {
    let all_series = series_repo::find_all(&state.db).await?;
    // series.metron_id -> (series_id, publisher) for tier-1 here.
    let by_metron: HashMap<String, (i64, Option<String>)> = all_series
        .into_iter()
        .filter_map(|s| s.metron_id.map(|m| (m, (s.id, s.publisher))))
        .collect();
    let on_list: HashSet<i64> = pull_list_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|e| e.series_id)
        .collect();

    let rows: Vec<CalendarRow> = items
        .into_iter()
        .map(|item| {
            let series_via_metron = item
                .metron_series_id
                .and_then(|msi| by_metron.get(&msi.to_string()).cloned());
            let (series_id, series_publisher) = match series_via_metron {
                Some((sid, pub_opt)) => (Some(sid), pub_opt),
                None => (None, None),
            };
            let publisher = series_publisher.or(item.publisher);
            let on_pull_list = series_id.is_some_and(|sid| on_list.contains(&sid));

            CalendarRow {
                cv_issue_id: item.cv_issue_id,
                cv_volume_id: None,
                metron_issue_id: Some(item.metron_issue_id),
                metron_series_id: item.metron_series_id,
                issue_number: item.issue_number,
                store_date: item.store_date.unwrap_or_default(),
                volume_name: item.series_name,
                cover_url: item.cover_url,
                site_detail_url: item.site_detail_url.unwrap_or_default(),
                series_id,
                on_pull_list,
                publisher,
            }
        })
        .collect();

    Ok(rows)
}

/// Whether a `metron_calendar_cache` row is still inside the TTL.
/// `cached_at` is stored UTC (`CURRENT_TIMESTAMP`); compare against UTC
/// now. TTL is a per-request settings read (default 24h).
fn is_metron_cache_fresh(cached_at: PrimitiveDateTime, ttl_hours: i64) -> bool {
    let now = OffsetDateTime::now_utc();
    let now_pdt = PrimitiveDateTime::new(now.date(), now.time());
    now_pdt - cached_at < time::Duration::hours(ttl_hours)
}

/// Whether a cache row is still inside [`CALENDAR_CACHE_TTL`]. `cached_at`
/// is stored UTC (`CURRENT_TIMESTAMP`); compare against UTC now.
fn is_fresh(cached_at: PrimitiveDateTime) -> bool {
    let now = OffsetDateTime::now_utc();
    let now_pdt = PrimitiveDateTime::new(now.date(), now.time());
    now_pdt - cached_at < CALENDAR_CACHE_TTL
}

/// The current comics shipping week — Wednesday through the following
/// Tuesday — as `(from, to)` `YYYY-MM-DD` strings, in UTC. New comics
/// ship Wednesday; `from` is the most-recent Wednesday at or before
/// today.
fn current_ship_week() -> (String, String) {
    let today = OffsetDateTime::now_utc().date();
    // number_days_from_monday(): Mon=0 .. Sun=6. Wednesday is 2.
    let days_from_monday = i64::from(today.weekday().number_days_from_monday());
    let back_to_wed = (days_from_monday + 7 - 2) % 7;
    let from = today - time::Duration::days(back_to_wed);
    let to = from + time::Duration::days(6);
    (fmt_date(from), fmt_date(to))
}

fn fmt_date(d: time::Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
}

/// A `YYYY-MM-DD`-shaped string. Not a full calendar check — a nonsense
/// month/day just yields an empty CV result — but it rejects empty /
/// wrong-shape input with a clean 400 instead of a confusing CV round
/// trip.
fn is_date_shaped(s: &str) -> bool {
    s.len() == 10
        && s.bytes().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}
