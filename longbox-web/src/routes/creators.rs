use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_comicvine::{CvPersonSearchResult, CvVolumeCredit};
use longbox_db::{
    creator_repo::{self, CreatorDetail, CreatorIssueRow, CreatorSearchRow},
    cv_volume_cache_repo::{self, CvVolumeMeta},
    publisher_filter_repo, series_repo,
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/creators/search", get(search_handler))
        .route("/creators/cv-search", get(cv_search_handler))
        .route("/creators/discover", get(discover_cv_handler))
        .route("/creators/:id", get(detail_handler))
        .route("/creators/:id/issues", get(issues_handler))
        .route("/creators/:id/discover", get(discover))
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: String,
}

async fn search_handler(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<CreatorSearchRow>>, ApiError> {
    let q = params.q.trim();
    if q.chars().count() < 2 {
        return Err(ApiError::BadRequest {
            message: "query parameter `q` must be at least 2 characters".into(),
        });
    }
    let rows = creator_repo::search_creators(&state.db, q).await?;
    Ok(Json(rows))
}

async fn detail_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<CreatorDetail>, ApiError> {
    match creator_repo::creator_detail(&state.db, id).await? {
        Some(detail) => Ok(Json(detail)),
        None => Err(ApiError::NotFound {
            resource: "creator",
            id: id.to_string(),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct IssuesParams {
    role: Option<String>,
    series_id: Option<i64>,
    #[serde(default = "default_page")]
    page: i64,
}

fn default_page() -> i64 {
    1
}

async fn issues_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<IssuesParams>,
) -> Result<Json<Vec<CreatorIssueRow>>, ApiError> {
    let rows = creator_repo::creator_issues(
        &state.db,
        id,
        params.role.as_deref(),
        params.series_id,
        params.page,
    )
    .await?;
    Ok(Json(rows))
}

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
/// cv_search.rs's `{ results, filtered_count }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct DiscoveryResponse {
    results: Vec<DiscoveredVolume>,
    filtered_count: u32,
}

/// Foreign-market reprint collection-line markers (lowercase substrings).
/// These are unambiguous foreign imprint lines (Panini España and similar) —
/// no English comic title contains them, so filtering on them is
/// zero-false-positive. Deterministic and extendable: add a marker when a real
/// foreign-reprint miss surfaces. Bare translated titles with no line marker
/// (e.g. "Doctor Extraño") have no safe signal here and are intentionally NOT
/// caught — those need publisher-level filtering (a deferred enrichment
/// follow-up), not a fragile language/accent heuristic.
const FOREIGN_COLLECTION_MARKERS: &[&str] = &[
    "100% marvel",
    "panini",
    "biblioteca marvel",
    "coleccionable",
    "grandes eventos marvel",
    "clásicos marvel",
    "clasicos marvel",
    "marvel deluxe",
    "marvel gold",
    "marvel must-have",
    "marvel imposible",
    // Accented "héroes" (Panini España line) — the accent means this can't
    // match the US unaccented "Marvel Heroes", so it stays zero-false-positive.
    "marvel héroes",
];

/// True when a volume name is a foreign-market reprint collection line.
fn is_foreign_reprint(name: &str) -> bool {
    let lower = name.to_lowercase();
    FOREIGN_COLLECTION_MARKERS.iter().any(|m| lower.contains(m))
}

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
        .filter(|c| !is_foreign_reprint(&c.name))
        .filter_map(|c| {
            let series_id = owned.get(&c.cv_volume_id).copied();
            let meta = cache_meta.get(&c.cv_volume_id);
            let publisher = meta.and_then(|m| m.publisher.clone());
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

    // Queue not-owned volumes for background enrichment — but skip static
    // foreign reprints, which build_discovery always drops, so we don't burn
    // a worker fetch_volume on a volume that will never render.
    let to_queue: Vec<i64> = credits
        .iter()
        .filter(|c| !owned_ids.contains(&c.cv_volume_id) && !is_foreign_reprint(&c.name))
        .map(|c| c.cv_volume_id)
        .collect();
    if let Err(e) = cv_volume_cache_repo::bulk_queue_pending(&state.db, &to_queue).await {
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
    Ok(DiscoveryResponse {
        results,
        filtered_count,
    })
}

#[derive(Debug, Deserialize)]
struct DiscoverParams {
    #[serde(default)]
    show_filtered: bool,
}

/// Discover by LOCAL creator id (existing route). Empty when the creator has
/// no known cv_person_id.
async fn discover(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<DiscoverParams>,
) -> Result<Json<DiscoveryResponse>, ApiError> {
    let Some(person_id) = creator_repo::cv_person_id_of(&state.db, id).await? else {
        return Ok(Json(DiscoveryResponse {
            results: Vec::new(),
            filtered_count: 0,
        }));
    };
    Ok(Json(
        discover_by_cv_person(&state, person_id, params.show_filtered).await?,
    ))
}

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
    #[serde(default)]
    show_filtered: bool,
}

async fn discover_cv_handler(
    State(state): State<AppState>,
    Query(params): Query<DiscoverCvParams>,
) -> Result<Json<DiscoveryResponse>, ApiError> {
    Ok(Json(
        discover_by_cv_person(&state, params.cv_person_id, params.show_filtered).await?,
    ))
}

#[cfg(test)]
mod discover_tests {
    use super::*;
    use longbox_comicvine::CvVolumeCredit;

    #[test]
    fn build_discovery_maps_owned_and_sorts_case_insensitive() {
        let credits = vec![
            CvVolumeCredit {
                cv_volume_id: 7084,
                name: "avengers".into(),
            },
            CvVolumeCredit {
                cv_volume_id: 999,
                name: "Deadly Class".into(),
            },
        ];
        // series id 3 owns cv volume 7084; 999 is not in the library
        let owned = vec![(3_i64, 7084_i64)];
        let (out, _) = build_discovery(credits, &owned, &HashMap::new(), &[]);
        assert_eq!(
            out,
            vec![
                DiscoveredVolume {
                    cv_volume_id: 7084,
                    name: "avengers".into(),
                    series_id: Some(3),
                    publisher: None,
                    start_year: None,
                    cover_url: None,
                },
                DiscoveredVolume {
                    cv_volume_id: 999,
                    name: "Deadly Class".into(),
                    series_id: None,
                    publisher: None,
                    start_year: None,
                    cover_url: None,
                },
            ]
        );
        // case-insensitive sort put lowercase "avengers" before "Deadly Class"
        assert_eq!(out[0].cv_volume_id, 7084);
    }

    #[test]
    fn build_discovery_enriches_and_publisher_filters() {
        use std::collections::HashMap;
        let credits = vec![
            CvVolumeCredit {
                cv_volume_id: 10,
                name: "Saga".into(),
            },
            CvVolumeCredit {
                cv_volume_id: 20,
                name: "Panini Reprint".into(),
            },
            CvVolumeCredit {
                cv_volume_id: 30,
                name: "Nailbiter".into(),
            },
            CvVolumeCredit {
                cv_volume_id: 40,
                name: "Blocked Book".into(),
            },
        ];
        let owned = vec![(3_i64, 10_i64)];
        let mut meta = HashMap::new();
        meta.insert(
            30,
            CvVolumeMeta {
                cv_volume_id: 30,
                publisher: Some("Image".into()),
                start_year: Some(2014),
                cover_url: Some("http://c/30.jpg".into()),
            },
        );
        meta.insert(
            40,
            CvVolumeMeta {
                cv_volume_id: 40,
                publisher: Some("Panini".into()),
                start_year: Some(2010),
                cover_url: None,
            },
        );
        let blocked = vec!["panini".to_string()];
        let (results, filtered) = build_discovery(credits, &owned, &meta, &blocked);
        assert_eq!(filtered, 1);
        let names: Vec<&str> = results.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, vec!["Nailbiter", "Saga"]);
        let saga = results.iter().find(|v| v.cv_volume_id == 10).unwrap();
        assert_eq!(saga.series_id, Some(3));
        let nail = results.iter().find(|v| v.cv_volume_id == 30).unwrap();
        assert_eq!(nail.publisher.as_deref(), Some("Image"));
        assert_eq!(nail.start_year, Some(2014));
        assert_eq!(nail.cover_url.as_deref(), Some("http://c/30.jpg"));
    }

    #[test]
    fn is_foreign_reprint_matches_markers_not_english_titles() {
        // Foreign collection lines -> filtered.
        assert!(is_foreign_reprint("100% Marvel. Caballero Luna"));
        assert!(is_foreign_reprint("Biblioteca Marvel: Spiderman"));
        assert!(is_foreign_reprint("Coleccionable Los Vengadores"));
        assert!(is_foreign_reprint("Marvel Deluxe. Lobezno"));
        assert!(is_foreign_reprint("Marvel Héroes: Lobezno")); // accented Panini line
                                                               // Real English titles from the live data -> NOT filtered (zero false positives).
        assert!(!is_foreign_reprint("Batman by Tom King Omnibus"));
        assert!(!is_foreign_reprint("Marvel Comics Presents"));
        assert!(!is_foreign_reprint("Action Comics"));
        assert!(!is_foreign_reprint("All-Star Batman"));
        assert!(!is_foreign_reprint("The Amazing Spider-Man"));
        // Unaccented US "Marvel Heroes" must NOT match (only the accented line).
        assert!(!is_foreign_reprint("Marvel Heroes"));
    }

    #[test]
    fn build_discovery_drops_foreign_reprints() {
        let credits = vec![
            CvVolumeCredit {
                cv_volume_id: 1,
                name: "Avengers".into(),
            },
            CvVolumeCredit {
                cv_volume_id: 2,
                name: "100% Marvel. Los Vengadores".into(),
            },
        ];
        let (out, _) = build_discovery(credits, &[], &HashMap::new(), &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Avengers"); // the foreign reprint was dropped
    }

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
        let map = vec![(52884_i64, 7_i64)];
        let out = flag_candidates(persons, &map);
        assert_eq!(out[0].in_library_creator_id, Some(7));
        assert_eq!(out[0].cv_person_id, 52884);
        assert_eq!(out[1].in_library_creator_id, None);
        assert_eq!(out.len(), 2);
    }
}
