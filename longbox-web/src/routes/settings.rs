use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use longbox_db::settings_repo;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(handler))
        .route("/settings/:key", put(update))
}

/// Reflects the boot-time env config (read-only) PLUS the four
/// runtime-tunable rows from `settings`. Consumers (scanner per scan,
/// enrichment per cycle, pull engine per sweep) already read each
/// tunable from the DB with a sensible fallback; this surface mirrors
/// what the user would see if they queried each consumer themselves.
///
/// `comicvine_api_key_configured` is structurally always `true` today —
/// the server refuses to start without a non-empty key. The field is
/// retained as a forward-compatible boolean so the wire contract doesn't
/// have to break later if an in-app key-setting flow is added.
#[derive(Debug, Serialize)]
struct SettingsResponse {
    library_root_path: String,
    database_url: String,
    bind_address: String,
    log_level: String,
    /// Boot-time env value. Historical field — kept for the existing
    /// frontend display row. The actually-load-bearing value is
    /// `match_confidence_threshold` below.
    match_threshold: f64,
    comicvine_api_key_configured: bool,
    /// Phase B's `DOWNLOAD_WATCH_PATH`. `None` = Phase B not
    /// configured for this deployment; otherwise the configured path
    /// (whether currently readable or not).
    download_watch_path: Option<String>,
    version: &'static str,

    /// Runtime-tunable: the scanner's owned-vs-needs_review threshold.
    /// Read per-scan via `Scanner::load_match_threshold`. Falls back
    /// to the boot env value when the DB row is missing.
    match_confidence_threshold: f64,
    /// Runtime-tunable: CV enrichment title-similarity gate for
    /// candidates with a known start year. Default 0.85.
    cv_enrichment_title_threshold_year_known: f64,
    /// Runtime-tunable: stricter title-similarity gate for candidates
    /// with no start year. Default 0.95.
    cv_enrichment_title_threshold_year_unknown: f64,
    /// Runtime-tunable: comma-separated list of release-title
    /// substrings the pull pre-grab filter drops. Empty string when
    /// the row is absent.
    pull_exclusion_keywords: String,
}

async fn handler(State(state): State<AppState>) -> Result<Json<SettingsResponse>, ApiError> {
    let match_confidence_threshold: f64 = settings_repo::get_or_default(
        &state.db,
        settings_repo::KEY_MATCH_CONFIDENCE_THRESHOLD,
        state.config.match_threshold,
    )
    .await?;
    let cv_enrichment_title_threshold_year_known: f64 = settings_repo::get_or_default(
        &state.db,
        settings_repo::KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_KNOWN,
        0.85,
    )
    .await?;
    let cv_enrichment_title_threshold_year_unknown: f64 = settings_repo::get_or_default(
        &state.db,
        settings_repo::KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_UNKNOWN,
        0.95,
    )
    .await?;
    let pull_exclusion_keywords: String = settings_repo::get_or_default(
        &state.db,
        settings_repo::KEY_PULL_EXCLUSION_KEYWORDS,
        String::new(),
    )
    .await?;

    Ok(Json(SettingsResponse {
        library_root_path: state.config.library_root_path.clone(),
        database_url: state.config.database_url.clone(),
        bind_address: state.config.bind_addr.clone(),
        log_level: state.config.log_level.clone(),
        match_threshold: state.config.match_threshold,
        comicvine_api_key_configured: !state.config.comicvine_api_key.is_empty(),
        download_watch_path: state.config.download_watch_path.clone(),
        version: env!("CARGO_PKG_VERSION"),
        match_confidence_threshold,
        cv_enrichment_title_threshold_year_known,
        cv_enrichment_title_threshold_year_unknown,
        pull_exclusion_keywords,
    }))
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    value: String,
}

#[derive(Debug, Serialize)]
struct UpdateResponse {
    key: String,
    value: String,
}

/// PUT /api/settings/:key with `{value: "..."}`. Whitelisted keys only —
/// boot-time env vars (library root, DB URL, bind addr, log level) are
/// not exposed to runtime mutation, and tweaking them requires a
/// container restart by design. Validation is per-key: thresholds are
/// f64 in [0.0, 1.0] (NaN/Inf rejected); the exclusion-keywords value
/// is stored verbatim — split/trim happens at the pull engine boundary
/// where the keys are consumed.
async fn update(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<UpdateResponse>, ApiError> {
    // Normalize the value: thresholds get parsed-and-restringified so
    // the DB row stores the canonical f64 form; keywords are stored as
    // typed. Validation happens against the canonical form.
    let stored_value: String = match key.as_str() {
        settings_repo::KEY_MATCH_CONFIDENCE_THRESHOLD
        | settings_repo::KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_KNOWN
        | settings_repo::KEY_CV_ENRICHMENT_TITLE_THRESHOLD_YEAR_UNKNOWN => {
            parse_threshold(&key, &body.value)?
        }
        settings_repo::KEY_PULL_EXCLUSION_KEYWORDS => body.value.clone(),
        _ => {
            return Err(ApiError::BadRequest {
                message: format!(
                    "setting '{key}' is not editable. Editable keys: \
                     match_confidence_threshold, \
                     cv_enrichment_title_threshold_year_known, \
                     cv_enrichment_title_threshold_year_unknown, \
                     pull_exclusion_keywords."
                ),
            });
        }
    };

    settings_repo::set(&state.db, &key, &stored_value).await?;
    Ok(Json(UpdateResponse {
        key,
        value: stored_value,
    }))
}

/// Parse a threshold string, returning the canonical f64 form (so the
/// DB stores `"0.85"` not `"0.85000000001"`). Rejects unparseable
/// values, NaN, Inf, and anything outside `[0.0, 1.0]`.
fn parse_threshold(key: &str, raw: &str) -> Result<String, ApiError> {
    let parsed: f64 = raw.trim().parse().map_err(|_| ApiError::BadRequest {
        message: format!("'{key}' must be a decimal number; got '{raw}'."),
    })?;
    if !parsed.is_finite() {
        return Err(ApiError::BadRequest {
            message: format!("'{key}' must be a finite number; got '{raw}'."),
        });
    }
    if !(0.0..=1.0).contains(&parsed) {
        return Err(ApiError::BadRequest {
            message: format!("'{key}' must be between 0.0 and 1.0 inclusive; got {parsed}."),
        });
    }
    Ok(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_accepts_zero_one_endpoints() {
        assert_eq!(parse_threshold("k", "0").unwrap(), "0");
        assert_eq!(parse_threshold("k", "1.0").unwrap(), "1");
        assert_eq!(parse_threshold("k", "0.85").unwrap(), "0.85");
    }

    #[test]
    fn threshold_trims_whitespace() {
        assert_eq!(parse_threshold("k", "  0.5  ").unwrap(), "0.5");
    }

    #[test]
    fn threshold_rejects_out_of_range() {
        assert!(parse_threshold("k", "1.01").is_err());
        assert!(parse_threshold("k", "-0.01").is_err());
        assert!(parse_threshold("k", "10").is_err());
    }

    #[test]
    fn threshold_rejects_nan_and_inf() {
        assert!(parse_threshold("k", "NaN").is_err());
        assert!(parse_threshold("k", "inf").is_err());
        assert!(parse_threshold("k", "-inf").is_err());
    }

    #[test]
    fn threshold_rejects_garbage() {
        assert!(parse_threshold("k", "banana").is_err());
        assert!(parse_threshold("k", "").is_err());
    }
}
