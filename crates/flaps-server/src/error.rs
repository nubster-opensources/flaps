//! API error type mapped to problem+json (RFC 9457).

use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

use flaps_store::StoreError;

/// All error outcomes the admin API can return, mapped to problem+json.
#[derive(Debug)]
pub enum ApiError {
    /// 401: missing or invalid authentication credentials.
    Unauthorized,
    /// 403: the authenticated principal is not permitted to access this resource.
    Forbidden,
    /// 422: request body is malformed or fails domain validation.
    InvalidBody(String),
    /// 400: the proposed change does not compile (invalid rules).
    Validation(flaps_compiler::CompileError),
    /// 404: the addressed resource does not exist.
    NotFound,
    /// 409: a uniqueness conflict (e.g. `external_ref` already used).
    Conflict(String),
    /// 412: the supplied If-Match does not match the current ETag.
    PreconditionFailed,
    /// 429: too many requests.
    TooManyRequests {
        /// Suggested wait time in seconds before the next request.
        retry_after_seconds: u64,
    },
    /// 500: an unexpected store or internal error.
    Internal(String),
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Conflict(msg) => Self::Conflict(msg),
            StoreError::NotFound => Self::NotFound,
            other => Self::Internal(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, type_suffix, title, detail, retry_after) = match &self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Unauthorized",
                "Missing or invalid authentication credentials.".to_owned(),
                None,
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "Forbidden",
                "This endpoint requires a server SDK key.".to_owned(),
                None,
            ),
            Self::InvalidBody(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid-body",
                "Invalid request body",
                msg.as_str().to_owned(),
                None,
            ),
            Self::Validation(err) => (
                StatusCode::BAD_REQUEST,
                "validation-error",
                "Compile validation failed",
                err.to_string(),
                None,
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not-found",
                "Resource not found",
                "The addressed resource does not exist.".to_owned(),
                None,
            ),
            Self::Conflict(msg) => (
                StatusCode::CONFLICT,
                "conflict",
                "Conflict",
                msg.as_str().to_owned(),
                None,
            ),
            Self::PreconditionFailed => (
                StatusCode::PRECONDITION_FAILED,
                "precondition-failed",
                "Precondition failed",
                "The supplied If-Match header does not match the current ETag.".to_owned(),
                None,
            ),
            Self::TooManyRequests {
                retry_after_seconds,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                "too-many-requests",
                "Too many requests",
                "Rate limit exceeded. Retry after the indicated delay.".to_owned(),
                Some(*retry_after_seconds),
            ),
            Self::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal-error",
                "Internal server error",
                msg.as_str().to_owned(),
                None,
            ),
        };

        let body = json!({
            "type": format!("https://flaps.dev/problems/{type_suffix}"),
            "title": title,
            "status": status.as_u16(),
            "detail": detail,
        });

        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();

        let mut builder = Response::builder().status(status).header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );

        if let Some(secs) = retry_after {
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                builder = builder.header("Retry-After", v);
            }
        }

        builder
            .body(axum::body::Body::from(body_bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    }
}
