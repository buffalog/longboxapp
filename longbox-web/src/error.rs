use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;
use thiserror::Error;
use tracing::{error, warn};

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("bad request: {message}")]
    BadRequest { message: String },

    #[error("{resource} not found: {id}")]
    NotFound { resource: &'static str, id: String },

    #[error("conflict ({code}): {message}")]
    Conflict { code: &'static str, message: String },

    #[error("validation failed: {} errors", errors.len())]
    Validation { errors: Vec<FieldError> },

    #[error("unprocessable ({code}): {message}")]
    Unprocessable { code: &'static str, message: String },

    #[error("upstream {service} failed (status {status}): {message}")]
    Upstream {
        service: &'static str,
        status: u16,
        message: String,
    },

    #[error("rate limited; retry after {retry_after_seconds}s")]
    RateLimited { retry_after_seconds: u64 },

    #[error("internal error: {message}")]
    Internal {
        message: String,
        #[source]
        source: anyhow::Error,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

impl ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Validation { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unprocessable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Upstream { .. } => StatusCode::BAD_GATEWAY,
            Self::RateLimited { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> String {
        match self {
            Self::BadRequest { .. } => "bad_request".into(),
            Self::NotFound { resource, .. } => format!("not_found.{resource}"),
            Self::Conflict { code, .. } => (*code).to_string(),
            Self::Validation { .. } => "validation".into(),
            Self::Unprocessable { code, .. } => (*code).to_string(),
            Self::Upstream { service, .. } => format!("upstream.{service}"),
            Self::RateLimited { .. } => "rate_limited".into(),
            Self::Internal { .. } => "internal".into(),
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::BadRequest { message } => message.clone(),
            Self::NotFound { resource, id } => format!("{resource} {id} not found"),
            Self::Conflict { message, .. } => message.clone(),
            Self::Validation { .. } => "Validation failed".into(),
            Self::Unprocessable { message, .. } => message.clone(),
            Self::Upstream {
                service, message, ..
            } => format!("Upstream {service} failed: {message}"),
            Self::RateLimited {
                retry_after_seconds,
            } => format!("Rate limited; retry after {retry_after_seconds}s"),
            // 500s deliberately hide the underlying message from the client.
            Self::Internal { .. } => "Internal server error".into(),
        }
    }

    fn details(&self) -> serde_json::Value {
        match self {
            Self::NotFound { resource, id } => json!({ "resource": resource, "id": id }),
            Self::Validation { errors } => json!({ "errors": errors }),
            Self::Upstream {
                service,
                status,
                message,
            } => json!({ "service": service, "status": status, "message": message }),
            Self::RateLimited {
                retry_after_seconds,
            } => json!({ "retry_after_seconds": retry_after_seconds }),
            _ => json!({}),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = json!({
            "error": {
                "code": self.code(),
                "message": self.user_message(),
                "details": self.details(),
            }
        });

        // Log before returning. 4xx at WARN, 5xx at ERROR with source chain.
        if status.is_server_error() {
            error!(
                target: "longbox_web",
                status = status.as_u16(),
                error = ?self,
                "server error"
            );
        } else {
            warn!(
                target: "longbox_web",
                status = status.as_u16(),
                error = %self,
                "client error"
            );
        }

        let mut headers = HeaderMap::new();
        if let Self::RateLimited {
            retry_after_seconds,
        } = &self
        {
            if let Ok(v) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
                headers.insert("retry-after", v);
            }
        }

        (status, headers, Json(body)).into_response()
    }
}

// -------- From impls --------

impl From<longbox_db::DbError> for ApiError {
    fn from(err: longbox_db::DbError) -> Self {
        match err {
            longbox_db::DbError::NotFound => ApiError::NotFound {
                resource: "row",
                id: "unspecified".into(),
            },
            longbox_db::DbError::UniqueViolation { field } => ApiError::Conflict {
                code: "conflict.unique_violation",
                message: format!("Unique constraint violated on field: {field}"),
            },
            longbox_db::DbError::MigrationFailed(m) => ApiError::Internal {
                message: format!("migration failed: {m}"),
                source: anyhow::anyhow!("MigrationFailed"),
            },
            longbox_db::DbError::MigrationGap { .. } => ApiError::Internal {
                message: err.to_string(),
                source: anyhow::anyhow!("MigrationGap"),
            },
            longbox_db::DbError::Other(e) => ApiError::Internal {
                message: format!("database error: {e}"),
                source: anyhow::anyhow!(e),
            },
        }
    }
}

impl From<longbox_comicvine::CvError> for ApiError {
    fn from(err: longbox_comicvine::CvError) -> Self {
        match err {
            longbox_comicvine::CvError::Auth => ApiError::Internal {
                message: "ComicVine credentials misconfigured".into(),
                source: anyhow::anyhow!("CvError::Auth"),
            },
            longbox_comicvine::CvError::NotFound => ApiError::NotFound {
                resource: "comicvine_volume",
                id: "unspecified".into(),
            },
            longbox_comicvine::CvError::RateLimited {
                retry_after_seconds,
            } => ApiError::RateLimited {
                retry_after_seconds,
            },
            longbox_comicvine::CvError::Http { status, body } => ApiError::Upstream {
                service: "comicvine",
                status,
                message: truncate(&body, 200).to_owned(),
            },
            longbox_comicvine::CvError::Network(e) => ApiError::Upstream {
                service: "comicvine",
                status: 0,
                message: format!("network: {e}"),
            },
            longbox_comicvine::CvError::Timeout => ApiError::Upstream {
                service: "comicvine",
                status: 0,
                message: "request timed out".into(),
            },
            longbox_comicvine::CvError::Malformed { message, .. } => ApiError::Upstream {
                service: "comicvine",
                status: 0,
                message,
            },
        }
    }
}

impl From<longbox_scanner::ScanError> for ApiError {
    fn from(err: longbox_scanner::ScanError) -> Self {
        match err {
            longbox_scanner::ScanError::AlreadyRunning => ApiError::Conflict {
                code: "conflict.scan_running",
                message: "Scan already in progress".into(),
            },
            longbox_scanner::ScanError::LibraryRootNotFound { id } => ApiError::NotFound {
                resource: "library_root",
                id: id.to_string(),
            },
            longbox_scanner::ScanError::Db(db_err) => ApiError::from(db_err),
            other => ApiError::Internal {
                message: format!("scanner: {other}"),
                source: anyhow::anyhow!("ScanError"),
            },
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_dberror_notfound_maps_to_404() {
        let err: ApiError = longbox_db::DbError::NotFound.into();
        assert_eq!(err.status_code(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn from_dberror_unique_maps_to_409() {
        let err: ApiError = longbox_db::DbError::UniqueViolation { field: "cv_id" }.into();
        assert_eq!(err.status_code(), StatusCode::CONFLICT);
    }

    #[test]
    fn from_scanerror_already_running_maps_to_409() {
        let err: ApiError = longbox_scanner::ScanError::AlreadyRunning.into();
        assert_eq!(err.status_code(), StatusCode::CONFLICT);
        assert_eq!(err.code(), "conflict.scan_running");
    }

    #[test]
    fn from_cverror_notfound_maps_to_404() {
        let err: ApiError = longbox_comicvine::CvError::NotFound.into();
        assert_eq!(err.status_code(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn from_cverror_ratelimited_carries_retry_after() {
        let err: ApiError = longbox_comicvine::CvError::RateLimited {
            retry_after_seconds: 42,
        }
        .into();
        assert_eq!(err.status_code(), StatusCode::SERVICE_UNAVAILABLE);
        match err {
            ApiError::RateLimited {
                retry_after_seconds,
            } => assert_eq!(retry_after_seconds, 42),
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn from_cverror_http_maps_to_upstream_502() {
        let err: ApiError = longbox_comicvine::CvError::Http {
            status: 503,
            body: "service unavailable".into(),
        }
        .into();
        assert_eq!(err.status_code(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn from_cverror_auth_maps_to_internal_500() {
        let err: ApiError = longbox_comicvine::CvError::Auth.into();
        assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
