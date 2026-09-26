//! Extractors whose rejections are rendered as [`ApiError`] JSON bodies.

use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use bytes::Bytes;
use http::request::Parts;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::error::ApiError;

/// JSON request body. Lenient about the `Content-Type` header (the CLI,
/// curl and the dashboard don't always agree on it); an empty body is read as
/// `{}` so endpoints whose fields are all optional accept a bare `POST`.
#[derive(Debug, Clone)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state).await?;
        parse_json(&bytes).map(ApiJson)
    }
}

/// Raw request body (subject to `DefaultBodyLimit`) with JSON rejections.
#[derive(Debug, Clone)]
pub struct ApiBytes(pub Bytes);

impl<S> FromRequest<S> for ApiBytes
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Ok(ApiBytes(Bytes::from_request(req, state).await?))
    }
}

/// Parse a JSON body (empty → `{}`) into `T`, with a 400 on failure.
pub fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    let trimmed = bytes.iter().all(u8::is_ascii_whitespace);
    let res = if trimmed { serde_json::from_slice(b"{}") } else { serde_json::from_slice(bytes) };
    res.map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))
}

/// Query string with a JSON 400 rejection.
#[derive(Debug, Clone, Default)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Query(v) = Query::<T>::try_from_uri(&parts.uri)?;
        Ok(ApiQuery(v))
    }
}

/// Path parameters with a JSON 400 rejection.
#[derive(Debug, Clone)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(v) = Path::<T>::from_request_parts(parts, state).await?;
        Ok(ApiPath(v))
    }
}

/// Parse a boolean flag leniently: `true/1/yes/on/""` → true, `false/0/no/off` → false.
pub fn parse_flag(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "1" | "true" | "yes" | "on" | "y" => Some(true),
        "0" | "false" | "no" | "off" | "n" => Some(false),
        _ => None,
    }
}

/// serde helper for boolean query flags (`?follow=1`, `?dry_run=true`, `?restart`).
pub fn de_flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    parse_flag(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid boolean '{s}'")))
}

/// Common `?follow=` query.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FollowQuery {
    #[serde(default, deserialize_with = "de_flag")]
    pub follow: bool,
}

/// `?restart=` for env var changes.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RestartQuery {
    #[serde(default, deserialize_with = "de_flag")]
    pub restart: bool,
}

/// `?force=&restart=` for deletions of resources other services depend on.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeleteQuery {
    /// Delete even though other services still use the resource.
    #[serde(default, deserialize_with = "de_flag")]
    pub force: bool,
    /// (env groups) restart the live services that were linked to it.
    #[serde(default, deserialize_with = "de_flag")]
    pub restart: bool,
}

/// `?limit=` for listings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LimitQuery {
    pub limit: Option<u32>,
}

impl LimitQuery {
    /// Requested limit (default 20), clamped to 1..=500.
    pub fn get(&self) -> u32 {
        self.limit.unwrap_or(20).clamp(1, 500)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        assert_eq!(parse_flag("TRUE"), Some(true));
        assert_eq!(parse_flag(""), Some(true));
        assert_eq!(parse_flag("0"), Some(false));
        assert_eq!(parse_flag("maybe"), None);
        let q: Query<FollowQuery> = Query::try_from_uri(&"/x?follow=1".parse().unwrap()).unwrap();
        assert!(q.0.follow);
        let q: Query<FollowQuery> = Query::try_from_uri(&"/x".parse().unwrap()).unwrap();
        assert!(!q.0.follow);
        assert!(Query::<FollowQuery>::try_from_uri(&"/x?follow=perhaps".parse().unwrap()).is_err());
    }

    #[test]
    fn json_bodies() {
        #[derive(Debug, Deserialize, Default)]
        struct Opt {
            a: Option<u32>,
        }
        assert_eq!(parse_json::<Opt>(b"").unwrap().a, None);
        assert_eq!(parse_json::<Opt>(b"  \n").unwrap().a, None);
        assert_eq!(parse_json::<Opt>(br#"{"a": 3}"#).unwrap().a, Some(3));
        let err = parse_json::<Opt>(b"{nope").unwrap_err();
        assert_eq!(err.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(LimitQuery { limit: Some(0) }.get(), 1);
        assert_eq!(LimitQuery { limit: None }.get(), 20);
        assert_eq!(LimitQuery { limit: Some(10_000) }.get(), 500);
    }
}
