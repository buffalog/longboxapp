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
/// when that series is subscribed.
#[derive(Debug, Serialize)]
struct CalendarRow {
    #[serde(flatten)]
    item: CvCalendarItem,
    series_id: Option<i64>,
    on_pull_list: bool,
}

#[derive(Debug, Deserialize)]
struct AddToPullBody {
    cv_volume_id: i64,
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

/// Compound "add to pull list": resolve the volume to a series (creating
/// it via ComicVine when LongBox doesn't track it yet — the Library Tidy
/// `add_or_get_from_cv` primitive), then subscribe it. Both halves are
/// idempotent, so a volume that is already a subscribed series is a
/// clean no-op `200`.
async fn add_to_pull_list(
    State(state): State<AppState>,
    Json(body): Json<AddToPullBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.cv_volume_id <= 0 {
        return Err(ApiError::BadRequest {
            message: "cv_volume_id must be > 0".into(),
        });
    }
    let (series, was_new) = add_or_get_from_cv(&state, body.cv_volume_id).await?;
    if was_new {
        // A freshly created series: match any of its files already on
        // disk, same as the /files and Library Tidy add paths.
        spawn_auto_rematch(&state, series.id, "calendar-add");
    }
    if pull_list_repo::get(&state.db, series.id).await?.is_none() {
        pull_list_repo::add(
            &state.db,
            NewPullEntry {
                series_id: series.id,
                start_issue: None,
            },
        )
        .await?;
    }
    Ok(Json(serde_json::json!({ "series_id": series.id })))
}

// -------- internals --------

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
async fn enrich(
    state: &AppState,
    items: Vec<CvCalendarItem>,
) -> Result<Vec<CalendarRow>, ApiError> {
    // cv_volume_id -> series_id, for every series LongBox tracks by cv_id.
    let by_cv: HashMap<i64, i64> = series_repo::find_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|s| s.cv_id.map(|cv| (cv, s.id)))
        .collect();
    let on_list: HashSet<i64> = pull_list_repo::list_all(&state.db)
        .await?
        .into_iter()
        .map(|e| e.series_id)
        .collect();
    Ok(items
        .into_iter()
        .map(|item| {
            let series_id = by_cv.get(&item.cv_volume_id).copied();
            let on_pull_list = series_id.is_some_and(|sid| on_list.contains(&sid));
            CalendarRow {
                item,
                series_id,
                on_pull_list,
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
