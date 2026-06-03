//! `webhook_configs` CRUD.
//!
//! `event_mask` is an INTEGER bitset of the `EVENT_*` flags defined in
//! `longbox_db::webhook_config_repo`; the handler rejects any bit
//! outside the known set so a client typo cannot persist a mask the
//! dispatcher will never read. The webhook URL is returned verbatim —
//! unlike an indexer key it is the user-chosen identifying field, not a
//! server-issued secret.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use longbox_db::webhook_config_repo::{self, EVENT_MASK_ALL};
use longbox_db::{NewWebhookConfig, WebhookConfigRow, WebhookConfigUpdate};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list).post(create))
        .route("/webhooks/:id", axum::routing::put(update).delete(remove))
        .route("/webhooks/:id/test", axum::routing::post(test))
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct WebhookBody {
    name: String,
    url: String,
    #[serde(default)]
    event_mask: i64,
    #[serde(default = "default_true")]
    enabled: bool,
}

/// Normalized, validated webhook fields.
struct ValidWebhook {
    name: String,
    url: String,
    event_mask: i64,
    enabled: bool,
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<WebhookConfigRow>>, ApiError> {
    Ok(Json(webhook_config_repo::list_all(&state.db).await?))
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<WebhookBody>,
) -> Result<Json<WebhookConfigRow>, ApiError> {
    let v = validate(body)?;
    webhook_config_repo::insert(
        &state.db,
        NewWebhookConfig {
            name: v.name.clone(),
            url: v.url,
            event_mask: v.event_mask,
            enabled: v.enabled,
        },
    )
    .await
    .map(Json)
    .map_err(|e| map_webhook_db_err(e, &v.name))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<WebhookBody>,
) -> Result<Json<WebhookConfigRow>, ApiError> {
    let v = validate(body)?;
    webhook_config_repo::update(
        &state.db,
        id,
        WebhookConfigUpdate {
            name: v.name.clone(),
            url: v.url,
            event_mask: v.event_mask,
            enabled: v.enabled,
        },
    )
    .await
    .map(Json)
    .map_err(|e| match e {
        longbox_db::DbError::NotFound => not_found(id),
        other => map_webhook_db_err(other, &v.name),
    })
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    webhook_config_repo::delete(&state.db, id)
        .await
        .map_err(|e| match e {
            longbox_db::DbError::NotFound => not_found(id),
            other => ApiError::from(other),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deliver a synthetic test notification to a webhook's URL, so the
/// user can verify the endpoint works before relying on it for real
/// events. A delivery failure surfaces as `422` rather than `500` — a
/// bad / unreachable URL is a client-fixable condition.
async fn test(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let webhook = webhook_config_repo::get(&state.db, id)
        .await?
        .ok_or_else(|| not_found(id))?;
    let event = longbox_webhooks::WebhookEvent {
        event: "test".into(),
        message: format!(
            "LongBox test notification for webhook \"{}\".",
            webhook.name
        ),
    };
    longbox_webhooks::deliver(&webhook.url, &event)
        .await
        .map_err(|e| ApiError::Unprocessable {
            code: "webhook.test_failed",
            message: format!("Test delivery failed: {e}"),
        })?;
    Ok(Json(serde_json::json!({ "delivered": true })))
}

fn validate(body: WebhookBody) -> Result<ValidWebhook, ApiError> {
    let name = body.name.trim().to_owned();
    if name.is_empty() {
        return Err(ApiError::BadRequest {
            message: "name must not be empty".into(),
        });
    }
    let url = body.url.trim().to_owned();
    if url.is_empty() {
        return Err(ApiError::BadRequest {
            message: "url must not be empty".into(),
        });
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError::BadRequest {
            message: "url must be an http(s) URL".into(),
        });
    }
    if body.event_mask & !EVENT_MASK_ALL != 0 {
        return Err(ApiError::BadRequest {
            message: format!(
                "event_mask has unknown bits set (known event mask is {EVENT_MASK_ALL})"
            ),
        });
    }
    Ok(ValidWebhook {
        name,
        url,
        event_mask: body.event_mask,
        enabled: body.enabled,
    })
}

fn not_found(id: i64) -> ApiError {
    ApiError::NotFound {
        resource: "webhook",
        id: id.to_string(),
    }
}

fn map_webhook_db_err(e: longbox_db::DbError, name: &str) -> ApiError {
    match e {
        longbox_db::DbError::UniqueViolation {
            field: "webhook_name",
        } => ApiError::Conflict {
            code: "conflict.webhook_exists",
            message: format!("A webhook named {name:?} already exists."),
            details: serde_json::Value::Null,
        },
        other => ApiError::from(other),
    }
}
