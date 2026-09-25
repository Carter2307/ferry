//! Bearer-token authentication for `/api/*`.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{Method, header};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::AppState;
use crate::error::ApiError;

/// Compare two secrets in constant time. Both sides are hashed first so that
/// neither the contents nor the length of the expected secret leak through
/// timing.
pub fn secrets_equal(provided: &str, expected: &str) -> bool {
    let a = Sha256::digest(provided.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    bool::from(a.as_slice().ct_eq(b.as_slice()))
}

/// Token from `Authorization: Bearer <token>`.
fn bearer_token(req: &Request) -> Option<String> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_string())
}

/// Token from `?access_token=` (GET only: EventSource can't set headers).
fn query_token(req: &Request) -> Option<String> {
    if req.method() != Method::GET {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Q {
        access_token: Option<String>,
    }
    let axum::extract::Query(q) = axum::extract::Query::<Q>::try_from_uri(req.uri()).ok()?;
    q.access_token
}

/// Middleware: require a valid API token.
pub async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let expected = state.config.api_token.as_str();
    if expected.is_empty() {
        // Fail closed: an unconfigured token must never mean "no auth".
        tracing::warn!("API request rejected: no API token is configured on the server");
        return ApiError::unauthorized("the server has no API token configured").into_response();
    }
    let provided = bearer_token(&req).or_else(|| query_token(&req));
    match provided {
        Some(token) if secrets_equal(&token, expected) => next.run(req).await,
        Some(_) => ApiError::unauthorized("invalid API token").into_response(),
        None => ApiError::unauthorized("missing API token (use 'Authorization: Bearer <token>')").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(secrets_equal("abc", "abc"));
        assert!(!secrets_equal("abc", "abd"));
        assert!(!secrets_equal("", "abc"));
        assert!(!secrets_equal("abcd", "abc"));
    }

    #[test]
    fn token_sources() {
        let req = Request::builder().header("authorization", "bearer  tok ").body(axum::body::Body::empty()).unwrap();
        assert_eq!(bearer_token(&req).as_deref(), Some("tok"));
        let req = Request::builder().header("authorization", "Basic xyz").body(axum::body::Body::empty()).unwrap();
        assert_eq!(bearer_token(&req), None);
        let req =
            Request::builder().uri("/api/v1/x?follow=1&access_token=a%2Bb").body(axum::body::Body::empty()).unwrap();
        assert_eq!(query_token(&req).as_deref(), Some("a+b"));
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/x?access_token=abc")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(query_token(&req), None);
    }
}
