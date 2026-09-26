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
use crate::locks;

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
/// content type — by not being a JSON `{"yaml": ...}` object. A body that
/// starts with `{` but isn't such an object (a YAML document in flow style,
/// or a blueprint written as JSON, which is YAML too) is raw YAML unless the
/// content type says JSON.
pub fn read_request(headers: &HeaderMap, body: &[u8], query_dry_run: bool) -> Result<(String, bool), ApiError> {
    let media = media_type(headers);
    if is_yaml(&media) {
        return Ok((utf8(body)?, query_dry_run));
    }
    let looks_like_json = body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    if is_json(&media) || looks_like_json {
        let json_err = match serde_json::from_slice::<ApplyBlueprint>(body) {
            Ok(req) => return Ok((req.yaml, req.dry_run || query_dry_run)),
            Err(e) => ApiError::bad_request(format!("invalid JSON body (expected {{\"yaml\": ...}}): {e}")),
        };
        if is_json(&media) || meant_as_request(body) {
            return Err(json_err);
        }
    }
    Ok((utf8(body)?, query_dry_run))
}

/// A body that parses as a mapping with an `ApplyBlueprint` key (`yaml`,
/// `dry_run`) was meant as the JSON request, so its error is the useful one.
fn meant_as_request(body: &[u8]) -> bool {
    match serde_yaml::from_slice::<serde_yaml::Value>(body) {
        Ok(serde_yaml::Value::Mapping(m)) => m.contains_key("yaml") || m.contains_key("dry_run"),
        // Not a YAML mapping either: report the JSON error.
        _ => true,
    }
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
    // Applies write many rows and queue deploys: run to completion even if
    // the client disconnects midway.
    locks::detached(async move {
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
    })
    .await
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
        // flow-style YAML (and JSON-written blueprints) sent as text are raw YAML
        let flow = b"{services: [{type: worker, name: w, image: busybox}]}";
        let (y, d) = read_request(&headers(Some("text/plain")), flow, true).unwrap();
        assert_eq!((y.as_bytes(), d), (&flow[..], true));
        let json_bp = br#"{"services": [{"type": "worker", "name": "w"}]}"#;
        assert_eq!(read_request(&headers(None), json_bp, false).unwrap().0.as_bytes(), json_bp);
        // ...but not with a JSON content type, and not when it was meant as the request
        assert!(read_request(&headers(Some("application/json")), flow, false).is_err());
        let err = read_request(&headers(None), br#"{"yaml": "a: 1", "dry_run": tru}"#, false).unwrap_err();
        assert!(err.body.error.message.contains("invalid JSON body"), "{err}");
        let err = read_request(&headers(None), br#"{"yaml": "a: 1", "extra": 1}"#, false).unwrap_err();
        assert!(err.body.error.message.contains("unknown field"), "{err}");
    }
}
