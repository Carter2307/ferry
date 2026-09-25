//! API error type: every failure is rendered as an [`ApiErrorBody`] JSON
//! document with the HTTP status of the underlying [`ferry_core::Error`].

use axum::Json;
use axum::extract::rejection::{BytesRejection, PathRejection, QueryRejection};
use axum::response::{IntoResponse, Response};
use ferry_core::dto::{ApiErrorBody, ApiErrorDetail};
use http::StatusCode;

/// Result alias for handlers.
pub type ApiResult<T> = Result<T, ApiError>;

/// An error response: status code + `{"error": {"code", "message"}}`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ApiErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            body: ApiErrorBody { error: ApiErrorDetail { code: code.to_string(), message: message.into() } },
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
    }

    pub fn method_not_allowed() -> Self {
        Self::new(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "method not allowed for this path")
    }

    /// Map a body-buffering rejection: 413 when the body limit was hit, else 400.
    pub fn from_bytes_rejection(r: BytesRejection) -> Self {
        if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Self::payload_too_large(format!("request body too large: {}", r.body_text()))
        } else {
            Self::bad_request(format!("failed to read request body: {}", r.body_text()))
        }
    }
}

impl From<ferry_core::Error> for ApiError {
    fn from(e: ferry_core::Error) -> Self {
        let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_server_error() {
            tracing::error!(error = %e, "request failed");
        } else {
            tracing::debug!(error = %e, status = status.as_u16(), "request rejected");
        }
        ApiError { status, body: ApiErrorBody::from(&e) }
    }
}

impl From<QueryRejection> for ApiError {
    fn from(r: QueryRejection) -> Self {
        Self::bad_request(format!("invalid query string: {}", r.body_text()))
    }
}

impl From<PathRejection> for ApiError {
    fn from(r: PathRejection) -> Self {
        Self::bad_request(format!("invalid path: {}", r.body_text()))
    }
}

impl From<BytesRejection> for ApiError {
    fn from(r: BytesRejection) -> Self {
        Self::from_bytes_rejection(r)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status.as_u16(), self.body.error.code, self.body.error.message)
    }
}
