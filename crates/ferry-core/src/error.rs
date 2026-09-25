//! Error type shared by all Ferry crates.

/// Result alias using [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every fallible Ferry operation returns this error. The variant decides the
/// HTTP status code the API maps it to (see [`Error::status`]).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A referenced entity does not exist (HTTP 404).
    #[error("{0} not found")]
    NotFound(String),
    /// The request is malformed or violates validation rules (HTTP 400).
    #[error("{0}")]
    Invalid(String),
    /// The request conflicts with current state, e.g. duplicate name (HTTP 409).
    #[error("{0}")]
    Conflict(String),
    /// Authentication failed (HTTP 401).
    #[error("{0}")]
    Unauthorized(String),
    /// SQLite / sqlx failure (HTTP 500).
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    /// Docker daemon failure (HTTP 502).
    #[error("docker error: {0}")]
    Docker(String),
    /// Image build failure (HTTP 500 when surfaced synchronously).
    #[error("build failed: {0}")]
    Build(String),
    /// Filesystem failure (HTTP 500).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The operation was canceled.
    #[error("canceled")]
    Canceled,
    /// Anything else (HTTP 500).
    #[error("{0}")]
    Internal(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// `Error::not_found("service", "web")` → "service 'web' not found".
    pub fn not_found(kind: &str, id: &str) -> Self {
        Error::NotFound(format!("{kind} '{id}'"))
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Error::Conflict(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Error::Internal(msg.into())
    }

    pub fn docker(msg: impl std::fmt::Display) -> Self {
        Error::Docker(msg.to_string())
    }

    /// Stable machine-readable error code used in API error bodies.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::Invalid(_) => "invalid_request",
            Error::Conflict(_) => "conflict",
            Error::Unauthorized(_) => "unauthorized",
            Error::Db(_) => "database_error",
            Error::Docker(_) => "docker_error",
            Error::Build(_) => "build_failed",
            Error::Io(_) => "io_error",
            Error::Canceled => "canceled",
            Error::Internal(_) | Error::Other(_) => "internal_error",
        }
    }

    /// HTTP status code for this error.
    pub fn status(&self) -> u16 {
        match self {
            Error::NotFound(_) => 404,
            Error::Invalid(_) => 400,
            Error::Conflict(_) => 409,
            Error::Unauthorized(_) => 401,
            Error::Docker(_) => 502,
            Error::Canceled => 409,
            _ => 500,
        }
    }
}
