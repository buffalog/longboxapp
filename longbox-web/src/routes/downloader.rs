//! `downloader_config` — the single-row Usenet downloader config.
//!
//! GET / PUT (upsert) / DELETE plus a connectivity test. The secret
//! (SABnzbd apikey or NZBGet control password) is masked on read
//! (`has_secret`, never the value) and write-only on update: a blank
//! `secret` in a PUT keeps whatever is stored.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use longbox_db::{downloader_config_repo, DownloaderConfigRow, NewDownloaderConfig};
use longbox_downloader::{connect, Downloader, DownloaderAuth, DownloaderConfig};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/downloader",
            get(get_config).put(put_config).delete(remove),
        )
        .route("/downloader/test", post(test))
}

/// The two supported downloader kinds — the `downloader_config.kind`
/// CHECK constraint enforces the same set at the DB layer.
const KINDS: [&str; 2] = ["sab", "nzbget"];

/// Read-facing projection — the secret itself is never serialized.
#[derive(Debug, Serialize)]
struct DownloaderView {
    kind: String,
    base_url: String,
    username: Option<String>,
    has_secret: bool,
    category: String,
    enabled: bool,
}

impl From<DownloaderConfigRow> for DownloaderView {
    fn from(r: DownloaderConfigRow) -> Self {
        Self {
            kind: r.kind,
            base_url: r.base_url,
            username: r.username,
            has_secret: !r.secret.is_empty(),
            category: r.category,
            enabled: r.enabled,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct DownloaderBody {
    kind: String,
    base_url: String,
    #[serde(default)]
    username: Option<String>,
    /// Blank/absent keeps the stored secret (valid only when a config
    /// already exists).
    #[serde(default)]
    secret: String,
    #[serde(default)]
    category: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct ConnectionTestResult {
    ok: bool,
    message: String,
}

async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<Option<DownloaderView>>, ApiError> {
    let row = downloader_config_repo::get(&state.db).await?;
    Ok(Json(row.map(DownloaderView::from)))
}

async fn put_config(
    State(state): State<AppState>,
    Json(body): Json<DownloaderBody>,
) -> Result<Json<DownloaderView>, ApiError> {
    let input = validate(&state, body).await?;
    let row = downloader_config_repo::upsert(&state.db, input).await?;
    Ok(Json(row.into()))
}

async fn remove(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    downloader_config_repo::clear(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Probe the downloader with the submitted form values. Always 200: a
/// failed connection reports `ok: false`, it is not an API error.
async fn test(
    State(state): State<AppState>,
    Json(body): Json<DownloaderBody>,
) -> Result<Json<ConnectionTestResult>, ApiError> {
    let input = validate(&state, body).await?;
    let cfg = to_client_config(&input);
    Ok(Json(match connect(&cfg).test_connection().await {
        Ok(()) => ConnectionTestResult {
            ok: true,
            message: "Downloader reachable and credentials accepted.".into(),
        },
        Err(e) => ConnectionTestResult {
            ok: false,
            message: e.to_string(),
        },
    }))
}

/// Validate + normalize a downloader body into a `NewDownloaderConfig`,
/// resolving a blank secret against the stored config.
async fn validate(state: &AppState, body: DownloaderBody) -> Result<NewDownloaderConfig, ApiError> {
    let kind = body.kind.trim().to_lowercase();
    if !KINDS.contains(&kind.as_str()) {
        return Err(ApiError::BadRequest {
            message: format!("kind must be 'sab' or 'nzbget', got {:?}", body.kind),
        });
    }
    let base_url = body.base_url.trim().to_owned();
    if base_url.is_empty() {
        return Err(ApiError::BadRequest {
            message: "base_url must not be empty".into(),
        });
    }
    // NZBGet authenticates with HTTP Basic — a username is mandatory.
    // SABnzbd uses an apikey only; any submitted username is dropped.
    let username = match kind.as_str() {
        "nzbget" => {
            let u = body.username.unwrap_or_default().trim().to_owned();
            if u.is_empty() {
                return Err(ApiError::BadRequest {
                    message: "username is required for NZBGet (HTTP Basic auth)".into(),
                });
            }
            Some(u)
        }
        _ => None,
    };
    let secret = resolve_secret(state, &body.secret).await?;
    Ok(NewDownloaderConfig {
        kind,
        base_url,
        username,
        secret,
        category: body.category.trim().to_owned(),
        enabled: body.enabled,
    })
}

/// A non-blank submitted secret replaces; a blank one keeps the stored
/// secret. Blank with nothing stored is a 400.
async fn resolve_secret(state: &AppState, submitted: &str) -> Result<String, ApiError> {
    let submitted = submitted.trim();
    if !submitted.is_empty() {
        return Ok(submitted.to_owned());
    }
    match downloader_config_repo::get(&state.db).await? {
        Some(row) if !row.secret.is_empty() => Ok(row.secret),
        _ => Err(ApiError::BadRequest {
            message: "secret is required (no stored downloader credentials to reuse)".into(),
        }),
    }
}

/// Map a validated config into the `longbox-downloader` client input.
/// `validate` guarantees `kind` is one of [`KINDS`].
fn to_client_config(input: &NewDownloaderConfig) -> DownloaderConfig {
    let auth = match input.kind.as_str() {
        "nzbget" => DownloaderAuth::Basic {
            username: input.username.clone().unwrap_or_default(),
            password: input.secret.clone(),
        },
        _ => DownloaderAuth::ApiKey(input.secret.clone()),
    };
    DownloaderConfig {
        base_url: input.base_url.clone(),
        auth,
        category: input.category.clone(),
    }
}
