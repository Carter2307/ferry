//! `POST /api/v1/blueprints/apply`.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{ApplyBlueprint, BlueprintResult};
use http::{HeaderMap, header};
use serde::Deserialize;

use crate::AppState;
use crate::blueprint;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiBytes, ApiQuery, de_flag};

/// Applies are serialized so two concurrent applies can't race on creating
/// the same resources.
static APPLY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Default, Deserialize)]
pub struct DryRunQuery {
    #[serde(default, deserialize_with = "de_flag")]
    pub dry_run: bool,
}

fn media_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_yaml(media: &str) -> bool {
    matches!(media, "application/yaml" | "text/yaml" | "application/x-yaml" | "text/x-yaml")
}

fn is_json(media: &str) -> bool {
    media == "application/json" || media.ends_with("+json")
}

fn utf8(body: &[u8]) -> Result<String, ApiError> {
    String::from_utf8(body.to_vec()).map_err(|_| ApiError::bad_request("the blueprint must be UTF-8 text"))
}

/// Decide between a JSON [`ApplyBlueprint`] body and raw YAML. Raw YAML is
/// recognized by its content type, or — for clients that send a generic
/// content type — by not being a JSON object.
pub fn read_request(headers: &HeaderMap, body: &[u8], query_dry_run: bool) -> Result<(String, bool), ApiError> {
    let media = media_type(headers);
    if is_yaml(&media) {
        return Ok((utf8(body)?, query_dry_run));
    }
    let looks_like_json = body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    if is_json(&media) || looks_like_json {
        let req: ApplyBlueprint = serde_json::from_slice(body)
            .map_err(|e| ApiError::bad_request(format!("invalid JSON body (expected {{\"yaml\": ...}}): {e}")))?;
        return Ok((req.yaml, req.dry_run || query_dry_run));
    }
    Ok((utf8(body)?, query_dry_run))
}

/// `POST /api/v1/blueprints/apply` — JSON `{yaml, dry_run}` or raw YAML with `?dry_run=`.
pub async fn apply(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<DryRunQuery>,
    headers: HeaderMap,
    ApiBytes(body): ApiBytes,
) -> ApiResult<Json<BlueprintResult>> {
    let (yaml, dry_run) = read_request(&headers, &body, q.dry_run)?;
    let bp = blueprint::parse(&yaml)?;
    let _guard = APPLY_LOCK.lock().await;
    let result = blueprint::apply(&st.store, &st.config, st.engine.as_ref(), &bp, dry_run).await?;
    tracing::info!(
        dry_run,
        actions = result.actions.len(),
        deploys = result.deploys.len(),
        warnings = result.warnings.len(),
        "applied blueprint"
    );
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn headers(ct: Option<&'static str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(ct) = ct {
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
        }
        h
    }

    #[test]
    fn body_formats() {
        let (y, d) = read_request(&headers(Some("application/yaml")), b"services: []", true).unwrap();
        assert_eq!((y.as_str(), d), ("services: []", true));
        let (y, d) =
            read_request(&headers(Some("application/json")), br#"{"yaml":"a: 1","dry_run":true}"#, false).unwrap();
        assert_eq!((y.as_str(), d), ("a: 1", true));
        let (y, d) = read_request(&headers(None), br#" {"yaml":"a: 1"}"#, true).unwrap();
        assert_eq!((y.as_str(), d), ("a: 1", true));
        // curl --data-binary defaults to a form content type
        let (y, _) =
            read_request(&headers(Some("application/x-www-form-urlencoded")), b"services:\n  - x", false).unwrap();
        assert_eq!(y, "services:\n  - x");
        assert!(read_request(&headers(Some("application/json")), b"services: []", false).is_err());
        assert!(read_request(&headers(Some("text/yaml; charset=utf-8")), &[0xff, 0xfe], false).is_err());
    }
}
