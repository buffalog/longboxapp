use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use longbox_comicvine::{CvPersonSearchResult, CvVolumeCredit};
use longbox_db::{
    creator_repo::{self, CreatorDetail, CreatorIssueRow, CreatorSearchRow},
    series_repo,
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
/// when the volume is already in the library (link to it), `None` when not
/// (offer to acquire via `POST /api/series {cv_id}`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct DiscoveredVolume {
    cv_volume_id: i64,
    name: String,
    series_id: Option<i64>,
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

/// Pure join+sort: map each CV volume credit to owned/not-owned against the
/// catalog's `(series_id, cv_id)` pairs, then sort by name case-insensitively.
fn build_discovery(
    credits: Vec<CvVolumeCredit>,
    owned_pairs: &[(i64, i64)],
) -> Vec<DiscoveredVolume> {
    let owned: HashMap<i64, i64> = owned_pairs
        .iter()
        .map(|(sid, cvid)| (*cvid, *sid))
        .collect();
    let mut out: Vec<DiscoveredVolume> = credits
        .into_iter()
        .filter(|c| !is_foreign_reprint(&c.name))
        .map(|c| DiscoveredVolume {
            series_id: owned.get(&c.cv_volume_id).copied(),
            cv_volume_id: c.cv_volume_id,
            name: c.name,
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
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
    Ok(Json(
        discover_by_cv_person(&state, params.cv_person_id).await?,
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
        let out = build_discovery(credits, &owned);
        assert_eq!(
            out,
            vec![
                DiscoveredVolume {
                    cv_volume_id: 7084,
                    name: "avengers".into(),
                    series_id: Some(3)
                },
                DiscoveredVolume {
                    cv_volume_id: 999,
                    name: "Deadly Class".into(),
                    series_id: None
                },
            ]
        );
        // case-insensitive sort put lowercase "avengers" before "Deadly Class"
        assert_eq!(out[0].cv_volume_id, 7084);
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
        let out = build_discovery(credits, &[]);
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
