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
use longbox_db::{
    downloader_config_repo, pull_attempt_repo, DownloaderConfigRow, NewDownloaderConfig,
};
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
        .route("/downloader/notify", post(notify))
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

/// SABnzbd/NZBGet post-processing notification body. Mirrors the
/// substitution variables a SAB script can pass back:
///
///   nzo_id   — job identifier (maps to `pull_attempts.download_handle`)
///   status   — SAB's vocabulary: "Completed" | "Failed" | "Warning"
///   fail_msg — optional fail_message string (present on failure)
#[derive(Debug, Deserialize)]
struct NotifyBody {
    nzo_id: String,
    /// SABnzbd status string. "Completed" is a no-op (Phase B handles
    /// the file landing in the watch folder). Anything else triggers
    /// failure processing.
    status: String,
    #[serde(default)]
    fail_msg: String,
}

/// SABnzbd/NZBGet post-processing webhook. Closes the gap between SAB
/// knowing a download failed and the next pull-sweep poll noticing —
/// up to 24h on the daily cadence. The script SAB invokes lives in
/// `SETUP.md`.
///
/// Contract: **always 200**. The downloader retries jobs whose
/// post-processing script returned an error code, and an alarming
/// status would cascade noise back to the user. Every branch — Completed,
/// unknown nzo_id, already-failed attempt, even a DB write that fails
/// inside `record_failure_if_submitted` — logs and returns 200.
async fn notify(State(state): State<AppState>, Json(body): Json<NotifyBody>) -> StatusCode {
    if body.status == "Completed" {
        // Phase B owns the success path — the file is landing in the
        // watch folder and the post-process pipeline will flip the
        // attempt to `grabbed` when it imports.
        return StatusCode::OK;
    }

    let attempt = match pull_attempt_repo::find_submitted_by_handle(&state.db, &body.nzo_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            tracing::debug!(
                target: "longbox_web",
                nzo_id = %body.nzo_id,
                status = %body.status,
                "notify.no_matching_attempt"
            );
            return StatusCode::OK;
        }
        Err(e) => {
            tracing::warn!(
                target: "longbox_web",
                nzo_id = %body.nzo_id,
                err = %e,
                "notify.lookup_failed"
            );
            return StatusCode::OK;
        }
    };

    let error_message = if body.fail_msg.trim().is_empty() {
        format!("SABnzbd status: {}", body.status)
    } else {
        body.fail_msg.clone()
    };

    if let Err(e) =
        pull_attempt_repo::record_failure_if_submitted(&state.db, attempt.id, &error_message).await
    {
        tracing::warn!(
            target: "longbox_web",
            attempt_id = attempt.id,
            err = %e,
            "notify.record_failure_failed"
        );
        return StatusCode::OK;
    }

    longbox_pull::fire_issue_search(state.db.clone(), attempt.series_id, attempt.issue_id);

    tracing::info!(
        target: "longbox_web",
        nzo_id = %body.nzo_id,
        series_id = attempt.series_id,
        issue_id = attempt.issue_id,
        error_message = %error_message,
        "notify.failure_processed"
    );
    StatusCode::OK
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
