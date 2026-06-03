//! `indexer_configs` CRUD + a connectivity test.
//!
//! The Newznab API key is masked on read (responses carry `has_api_key`,
//! never the value) and write-only on update: a blank `api_key` in a PUT
//! keeps whatever is stored. See the Phase A.8 Step 5 kickoff.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{indexer_config_repo, IndexerConfigRow, IndexerConfigUpdate, NewIndexerConfig};
use longbox_newznab::{IndexerConfig, IndexerId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/indexers", get(list).post(create))
        .route("/indexers/test", post(test))
        .route("/indexers/:id", axum::routing::put(update).delete(remove))
}

/// Read-facing projection of an indexer row — the API key itself is
/// never serialized, only whether one is set.
#[derive(Debug, Serialize)]
struct IndexerView {
    id: i64,
    name: String,
    base_url: String,
    has_api_key: bool,
    enabled: bool,
    priority: i64,
    maxage_days: i64,
}

impl From<IndexerConfigRow> for IndexerView {
    fn from(r: IndexerConfigRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            base_url: r.base_url,
            has_api_key: !r.api_key.is_empty(),
            enabled: r.enabled,
            priority: r.priority,
            maxage_days: r.maxage_days,
        }
    }
}

fn default_maxage() -> i64 {
    1500
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct IndexerBody {
    name: String,
    base_url: String,
    /// Blank/absent keeps the stored key on update; required on create.
    #[serde(default)]
    api_key: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    priority: i64,
    #[serde(default = "default_maxage")]
    maxage_days: i64,
}

#[derive(Debug, Deserialize)]
struct IndexerTestBody {
    /// When set with a blank `api_key`, the stored key for this row is
    /// used — re-test an existing indexer without re-typing the key.
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    name: String,
    base_url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default = "default_maxage")]
    maxage_days: i64,
}

#[derive(Debug, Serialize)]
struct ConnectionTestResult {
    ok: bool,
    message: String,
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<IndexerView>>, ApiError> {
    let rows = indexer_config_repo::list_all(&state.db).await?;
    Ok(Json(rows.into_iter().map(IndexerView::from).collect()))
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<IndexerBody>,
) -> Result<Json<IndexerView>, ApiError> {
    let name = body.name.trim().to_owned();
    let base_url = body.base_url.trim().to_owned();
    require_non_empty(&name, "name")?;
    require_non_empty(&base_url, "base_url")?;
    let api_key = body.api_key.trim().to_owned();
    if api_key.is_empty() {
        return Err(ApiError::BadRequest {
            message: "api_key is required when adding an indexer".into(),
        });
    }
    let row = indexer_config_repo::insert(
        &state.db,
        NewIndexerConfig {
            name: name.clone(),
            base_url,
            api_key,
            enabled: body.enabled,
            priority: body.priority,
            maxage_days: body.maxage_days,
        },
    )
    .await
    .map_err(|e| map_indexer_db_err(e, &name))?;
    Ok(Json(row.into()))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<IndexerBody>,
) -> Result<Json<IndexerView>, ApiError> {
    let name = body.name.trim().to_owned();
    let base_url = body.base_url.trim().to_owned();
    require_non_empty(&name, "name")?;
    require_non_empty(&base_url, "base_url")?;
    // Blank api_key on update keeps the stored key.
    let api_key = resolve_indexer_key(&state, id, &body.api_key).await?;
    let row = indexer_config_repo::update(
        &state.db,
        id,
        IndexerConfigUpdate {
            name: name.clone(),
            base_url,
            api_key,
            enabled: body.enabled,
            priority: body.priority,
            maxage_days: body.maxage_days,
        },
    )
    .await
    .map_err(|e| match e {
        longbox_db::DbError::NotFound => not_found(id),
        other => map_indexer_db_err(other, &name),
    })?;
    Ok(Json(row.into()))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    indexer_config_repo::delete(&state.db, id)
        .await
        .map_err(|e| match e {
            longbox_db::DbError::NotFound => not_found(id),
            other => ApiError::from(other),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Probe an indexer with the submitted form values — verifies the key
/// *before* it is committed. Always 200: a failed connection is a
/// successful test reporting `ok: false`, not an API error.
async fn test(
    State(state): State<AppState>,
    Json(body): Json<IndexerTestBody>,
) -> Result<Json<ConnectionTestResult>, ApiError> {
    let base_url = body.base_url.trim().to_owned();
    require_non_empty(&base_url, "base_url")?;
    let api_key = match (body.api_key.trim(), body.id) {
        (k, _) if !k.is_empty() => k.to_owned(),
        (_, Some(id)) => stored_indexer_key(&state, id).await?,
        (_, None) => {
            return Err(ApiError::BadRequest {
                message: "api_key is required to test a new indexer".into(),
            })
        }
    };
    let name = body.name.trim();
    let cfg = IndexerConfig {
        id: IndexerId(body.id.unwrap_or(0)),
        name: if name.is_empty() {
            "indexer".into()
        } else {
            name.to_owned()
        },
        base_url,
        api_key,
        priority: 0,
        maxage_days: clamp_maxage(body.maxage_days),
    };
    Ok(Json(match longbox_newznab::test_connection(&cfg).await {
        Ok(()) => ConnectionTestResult {
            ok: true,
            message: "Indexer reachable and API key accepted.".into(),
        },
        Err(e) => ConnectionTestResult {
            ok: false,
            message: e.to_string(),
        },
    }))
}

fn require_non_empty(value: &str, field: &str) -> Result<(), ApiError> {
    if value.is_empty() {
        return Err(ApiError::BadRequest {
            message: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

fn not_found(id: i64) -> ApiError {
    ApiError::NotFound {
        resource: "indexer",
        id: id.to_string(),
    }
}

/// Resolve the api_key to persist on update: a non-blank submitted key
/// replaces; a blank one keeps whatever is stored.
async fn resolve_indexer_key(
    state: &AppState,
    id: i64,
    submitted: &str,
) -> Result<String, ApiError> {
    let submitted = submitted.trim();
    if !submitted.is_empty() {
        return Ok(submitted.to_owned());
    }
    stored_indexer_key(state, id).await
}

async fn stored_indexer_key(state: &AppState, id: i64) -> Result<String, ApiError> {
    indexer_config_repo::get(&state.db, id)
        .await?
        .map(|r| r.api_key)
        .ok_or_else(|| not_found(id))
}

fn map_indexer_db_err(e: longbox_db::DbError, name: &str) -> ApiError {
    match e {
        longbox_db::DbError::UniqueViolation {
            field: "indexer_name",
        } => ApiError::Conflict {
            code: "conflict.indexer_exists",
            message: format!("An indexer named {name:?} already exists."),
            details: serde_json::Value::Null,
        },
        other => ApiError::from(other),
    }
}

/// `IndexerConfig.maxage_days` is `u32`; clamp a stray negative or
/// out-of-range value back to the default rather than rejecting.
fn clamp_maxage(days: i64) -> u32 {
    u32::try_from(days).unwrap_or(1500)
}
