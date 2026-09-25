//! Webhooks: secret deploy hooks and GitHub push events. No bearer token.

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ferry_core::{Deploy, DeployRequest, DeployTrigger, git};
use hmac::{Hmac, Mac};
use http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;

use crate::AppState;
use crate::auth::secrets_equal;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiBytes, ApiPath, ApiQuery};

#[derive(Debug, Default, Deserialize)]
pub struct HookQuery {
    pub key: Option<String>,
}

/// `GET|POST /hooks/deploy/{service_id}?key=`
pub async fn deploy_hook(
    State(st): State<AppState>,
    ApiPath(service_id): ApiPath<String>,
    ApiQuery(q): ApiQuery<HookQuery>,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    let svc = st.store.require_service(&service_id).await?;
    let key = q.key.unwrap_or_default();
    if key.is_empty() || !secrets_equal(&key, &svc.deploy_hook_key) {
        return Err(ApiError::unauthorized("invalid deploy hook key"));
    }
    let d = st.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::DeployHook)).await?;
    tracing::info!(service = %svc.name, deploy = %d.id, "deploy hook triggered a deploy");
    Ok((StatusCode::ACCEPTED, Json(d)))
}

type HmacSha256 = Hmac<Sha256>;

/// Verify `X-Hub-Signature-256: sha256=<hex>` over the raw body (constant time).
pub fn verify_github_signature(secret: &str, body: &[u8], header: Option<&str>) -> bool {
    let Some(sig) = header.map(str::trim).and_then(|h| h.strip_prefix("sha256=")) else {
        return false;
    };
    let Ok(sig) = hex::decode(sig) else { return false };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// `POST /hooks/github`
pub async fn github(State(st): State<AppState>, headers: HeaderMap, ApiBytes(body): ApiBytes) -> ApiResult<Response> {
    let Some(secret) = st.config.github_webhook_secret.as_deref().filter(|s| !s.is_empty()) else {
        return Err(ApiError::not_found(
            "GitHub webhooks are not enabled on this server (no webhook secret configured)",
        ));
    };
    let signature = header_str(&headers, "x-hub-signature-256").map(str::to_string);
    let valid = if body.len() > 1024 * 1024 {
        // Hash big payloads off the async worker threads.
        let (secret, body) = (secret.to_string(), body.clone());
        tokio::task::spawn_blocking(move || verify_github_signature(&secret, &body, signature.as_deref()))
            .await
            .map_err(|e| ferry_core::Error::internal(format!("verifying the webhook signature: {e}")))?
    } else {
        verify_github_signature(secret, &body, signature.as_deref())
    };
    if !valid {
        tracing::warn!("rejected GitHub webhook with a missing or invalid signature");
        return Err(ApiError::unauthorized("invalid or missing X-Hub-Signature-256"));
    }
    let event = header_str(&headers, "x-github-event").unwrap_or("").trim().to_ascii_lowercase();
    match event.as_str() {
        "ping" => Ok(Json(json!({"ok": true})).into_response()),
        "push" => {
            let payload: Value = serde_json::from_slice(&body)
                .map_err(|e| ApiError::bad_request(format!("invalid GitHub push payload: {e}")))?;
            let deploys = handle_push(&st, &payload).await?;
            Ok(Json(json!({ "deploys": deploys })).into_response())
        }
        _ => Ok(Json(json!({"ignored": true})).into_response()),
    }
}

/// Deploy every auto-deploy service tracking the pushed repository + branch.
async fn handle_push(st: &AppState, payload: &Value) -> ApiResult<Vec<Deploy>> {
    let Some(branch) = payload.get("ref").and_then(Value::as_str).and_then(|r| r.strip_prefix("refs/heads/")) else {
        tracing::debug!("ignoring push to a non-branch ref");
        return Ok(Vec::new());
    };
    if payload.get("deleted").and_then(Value::as_bool).unwrap_or(false) {
        tracing::debug!(branch, "ignoring push that deleted a branch");
        return Ok(Vec::new());
    }
    let commit = payload
        .get("after")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|c| !c.is_empty() && c.bytes().any(|b| b != b'0'))
        .map(str::to_string);
    let repo = payload.get("repository");
    let urls: Vec<String> = ["clone_url", "ssh_url", "git_url", "html_url", "url"]
        .iter()
        .filter_map(|k| repo.and_then(|r| r.get(*k)).and_then(Value::as_str))
        .filter(|u| !u.trim().is_empty())
        .map(git::normalize_repo_url)
        .collect();
    if urls.is_empty() {
        return Err(ApiError::bad_request("push payload has no repository URL"));
    }

    let mut deploys = Vec::new();
    for svc in st.store.list_services().await? {
        let Some(repo_url) = &svc.repo_url else { continue };
        if !svc.auto_deploy || svc.branch != branch || !urls.contains(&git::normalize_repo_url(repo_url)) {
            continue;
        }
        if svc.suspended {
            tracing::info!(service = %svc.name, "push matches a suspended service; not deploying");
            continue;
        }
        let req =
            DeployRequest { trigger: DeployTrigger::Webhook, source: None, commit: commit.clone(), clear_cache: false };
        match st.engine.deploy(&svc.id, req).await {
            Ok(d) => {
                tracing::info!(service = %svc.name, deploy = %d.id, branch, "git push triggered a deploy");
                deploys.push(d);
            }
            Err(e) => tracing::warn!(service = %svc.name, "deploy after git push failed: {e}"),
        }
    }
    Ok(deploys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        // Example from GitHub's documentation.
        let secret = "It's a Secret to Everybody";
        let body = b"Hello, World!";
        let good = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify_github_signature(secret, body, Some(good)));
        assert!(!verify_github_signature(secret, b"Hello, World?", Some(good)));
        assert!(!verify_github_signature("other", body, Some(good)));
        assert!(!verify_github_signature(secret, body, None));
        assert!(!verify_github_signature(secret, body, Some("sha1=abc")));
        assert!(!verify_github_signature(secret, body, Some("sha256=zz")));
        assert!(!verify_github_signature(secret, body, Some("sha256=")));
    }
}
