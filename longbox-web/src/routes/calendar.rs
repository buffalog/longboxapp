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
use longbox_db::{
    pull_list_repo, release_cache_repo, series_repo, NewPullEntry, NewReleaseCacheEntry,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::error::ApiError;
use crate::routes::series::{add_or_get_from_cv, spawn_auto_rematch};
use crate::state::AppState;

/// How long a cached CV release-calendar payload stays fresh. A user
/// query inside this window is served from `cv_release_cache`; past it
/// (or with `?refresh=true`) the handler re-queries ComicVine.
const CALENDAR_CACHE_TTL: time::Duration = time::Duration::hours(1);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/releases/calendar", get(calendar))
        .route("/releases/calendar/pull", post(add_to_pull_list))
        .route("/releases/calendar/pull/bulk", post(add_to_pull_list_bulk))
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

/// A calendar issue plus the live pull-list enrichment. `series_id` is
/// set when the issue's volume maps to a tracked series; `on_pull_list`
/// when that series is subscribed; `publisher` carries the tracked
/// series's `publisher` column (sourced from the 6c.5 enrichment merge
/// and refresh-pass), because CV's `/issues/` query never returns
/// publisher data. Untracked volumes get `publisher: null` — the
/// frontend groups them under "Unknown Publisher".
#[derive(Debug, Serialize)]
struct CalendarRow {
    #[serde(flatten)]
    item: CvCalendarItem,
    series_id: Option<i64>,
    on_pull_list: bool,
    publisher: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddToPullBody {
    cv_volume_id: i64,
}

#[derive(Debug, Deserialize)]
struct BulkAddBody {
    cv_volume_ids: Vec<i64>,
}

/// Per-volume outcome of a bulk add. `status` is one of `added`,
/// `already_on_list`, or `failed`; `series_id` rides along on the two
/// success statuses, `error` on `failed`.
#[derive(Debug, Serialize)]
struct BulkAddResult {
    cv_volume_id: i64,
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

/// The release calendar for a date range. Cache-aware: a fresh
/// `cv_release_cache` row is served as-is; otherwise CV is queried and
/// the result re-cached. Either way the rows are enriched live with
/// pull-list state before returning.
async fn calendar(
    State(state): State<AppState>,
    Query(q): Query<CalendarQuery>,
) -> Result<Json<Vec<CalendarRow>>, ApiError> {
    if !is_date_shaped(&q.from) || !is_date_shaped(&q.to) {
        return Err(ApiError::BadRequest {
            message: "`from` and `to` must be YYYY-MM-DD dates".into(),
        });
    }
    let items = load_calendar(&state, &q.from, &q.to, q.refresh).await?;
    Ok(Json(enrich(&state, items).await?))
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
        "added"
    } else {
        "already_on_list"
    };
    Ok((series.id, status))
}

/// Compound "add to pull list": resolve the volume to a series (creating
/// it via ComicVine when LongBox doesn't track it yet), then subscribe
/// it. Idempotent — a volume already subscribed is a clean no-op `200`.
async fn add_to_pull_list(
    State(state): State<AppState>,
    Json(body): Json<AddToPullBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (series_id, _status) = try_add_one(&state, body.cv_volume_id).await?;
    Ok(Json(serde_json::json!({ "series_id": series_id })))
}

/// Bulk "add to pull list" — one [`try_add_one`] per volume,
/// non-transactional: a per-volume CV failure (rate limit, unknown
/// volume, network) is captured as a `failed` result and never aborts
/// the batch. Each result carries the 3-way `status` the UI tallies
/// into one "N added, M already on pull list, K failed" toast.
async fn add_to_pull_list_bulk(
    State(state): State<AppState>,
    Json(body): Json<BulkAddBody>,
) -> Result<Json<BulkAddResponse>, ApiError> {
    let mut results = Vec::with_capacity(body.cv_volume_ids.len());
    for cv_volume_id in body.cv_volume_ids {
        let result = match try_add_one(&state, cv_volume_id).await {
            Ok((series_id, status)) => BulkAddResult {
                cv_volume_id,
                status,
                series_id: Some(series_id),
                error: None,
            },
            Err(e) => BulkAddResult {
                cv_volume_id,
                status: "failed",
                series_id: None,
                error: Some(e.to_string()),
            },
        };
        results.push(result);
    }
    Ok(Json(BulkAddResponse { results }))
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
    let items = load_calendar(&state, &from, &to, false).await?;

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
    let items = load_calendar(&state, &from, &to, false).await?;
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
async fn load_calendar(
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

/// Enrich CV calendar items with live tracked-series / pull-list state.
/// The `by_cv` map carries `(series_id, publisher)` per CV volume id —
/// publisher is the 6c.5 JOIN that powers Item E's publisher grouping
/// in the calendar UI. CV's `/issues/` query returns no publisher, so
/// without this JOIN every calendar row would be "Unknown Publisher".
async fn enrich(
    state: &AppState,
    items: Vec<CvCalendarItem>,
) -> Result<Vec<CalendarRow>, ApiError> {
    // cv_volume_id -> (series_id, publisher), for every series LongBox
    // tracks by cv_id. publisher is None for CV-linked series whose
    // refresh-pass hasn't run yet — the frontend groups those under
    // "Unknown Publisher" until the backfill completes.
    let by_cv: HashMap<i64, (i64, Option<String>)> = series_repo::find_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|s| s.cv_id.map(|cv| (cv, (s.id, s.publisher))))
        .collect();
    let on_list: HashSet<i64> = pull_list_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|e| e.series_id)
        .collect();
    Ok(items
        .into_iter()
        .map(|item| {
            let entry = by_cv.get(&item.cv_volume_id);
            let series_id = entry.map(|(sid, _)| *sid);
            let publisher = entry.and_then(|(_, pub_opt)| pub_opt.clone());
            let on_pull_list = series_id.is_some_and(|sid| on_list.contains(&sid));
            CalendarRow {
                item,
                series_id,
                on_pull_list,
                publisher,
            }
        })
        .collect())
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
