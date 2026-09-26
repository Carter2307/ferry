//! Webhooks: secret deploy hooks and GitHub push events. No bearer token.
//!
//! Responses never reveal more than the caller already proved it may know:
//! the deploy hook answers the same 401 for unknown services and wrong keys,
//! and returned deploys have credentials redacted ([`public_deploy`]).

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ferry_core::{Deploy, DeployRequest, DeployTrigger, Store, git, validate};
use hmac::{Hmac, Mac};
use http::{HeaderMap, StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::auth::secrets_equal;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiBytes, ApiPath, ApiQuery};
use crate::views::public_deploy;

#[derive(Debug, Default, Deserialize)]
pub struct HookQuery {
    pub key: Option<String>,
}

/// `GET|POST /hooks/deploy/{service_id}?key=` — the path takes the service
/// **id** only (names are guessable); an unknown id and a wrong key get the
/// same 401, so the hook can't be used to discover services.
pub async fn deploy_hook(
    State(st): State<AppState>,
    ApiPath(service_id): ApiPath<String>,
    ApiQuery(q): ApiQuery<HookQuery>,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    let svc = st.store.get_service(service_id.trim()).await?;
    let key = q.key.unwrap_or_default();
    // Compare against something even for unknown ids (similar timing).
    let expected = svc.as_ref().map_or("", |s| s.deploy_hook_key.as_str());
    let valid = secrets_equal(&key, expected) && !key.is_empty() && !expected.is_empty();
    let Some(svc) = svc.filter(|_| valid) else {
        return Err(ApiError::unauthorized("invalid deploy hook URL or key"));
    };
    let d = st.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::DeployHook)).await?;
    tracing::info!(service = %svc.name, deploy = %d.id, "deploy hook triggered a deploy");
    Ok((StatusCode::ACCEPTED, Json(public_deploy(d))))
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

/// The JSON payload of a delivery: the body itself (content type
/// `application/json`) or its `payload` field (GitHub's default content type
/// `application/x-www-form-urlencoded`). The signature covers the raw body
/// either way.
fn github_payload(headers: &HeaderMap, body: &[u8]) -> Result<Value, ApiError> {
    let media = header_str(headers, header::CONTENT_TYPE.as_str())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let form = media == "application/x-www-form-urlencoded" || (media.is_empty() && body.starts_with(b"payload="));
    let json: std::borrow::Cow<'_, [u8]> = if form {
        let payload = form_urlencoded::parse(body).find(|(k, _)| k == "payload").map(|(_, v)| v.into_owned());
        let Some(payload) = payload else {
            return Err(ApiError::bad_request(
                "invalid GitHub payload: the form-encoded body has no 'payload' field (set the webhook's content type to application/json)",
            ));
        };
        std::borrow::Cow::Owned(payload.into_bytes())
    } else {
        std::borrow::Cow::Borrowed(body)
    };
    serde_json::from_slice(&json).map_err(|e| ApiError::bad_request(format!("invalid GitHub push payload: {e}")))
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
            let payload = github_payload(&headers, &body)?;
            let delivery = header_str(&headers, "x-github-delivery").unwrap_or("-").to_string();
            // Finish (deploys + replay bookkeeping) even if GitHub hangs up.
            let body_hash = hex::encode(Sha256::digest(&body));
            let outcome = crate::locks::detached(async move { handle_push(&st, &payload, &body_hash).await }).await?;
            match outcome {
                PushOutcome::Deployed(deploys) => {
                    let deploys: Vec<Deploy> = deploys.into_iter().map(public_deploy).collect();
                    Ok(Json(json!({ "deploys": deploys })).into_response())
                }
                PushOutcome::Ignored(reason) => {
                    tracing::info!(%delivery, "ignored GitHub push: {reason}");
                    Ok(Json(json!({ "deploys": [], "ignored": true, "reason": reason })).into_response())
                }
            }
        }
        _ => Ok(Json(json!({"ignored": true})).into_response()),
    }
}

enum PushOutcome {
    Deployed(Vec<Deploy>),
    Ignored(String),
}

/// Settings key of the replay-protection state.
const PUSH_STATE_KEY: &str = "github_push_state";

/// Entries kept per list of [`PushState`].
const PUSH_HISTORY: usize = 256;

/// Serializes the read-modify-write of [`PushState`].
static PUSH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What protects `/hooks/github` against replays. The HMAC covers only the
/// body (no timestamp) and `X-GitHub-Delivery` isn't signed, so:
/// * `payloads`: hashes of recently processed push bodies — the exact same
///   signed body is never processed twice (replays, duplicate deliveries);
/// * `superseded`: `repo|branch|sha` of commits a later push moved away from
///   (its `before`) — an older push replayed or delivered late is ignored
///   instead of rolling the service back. Force pushes and branch creations
///   may legitimately move back, so they are exempt.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PushState {
    #[serde(default)]
    payloads: Vec<String>,
    #[serde(default)]
    superseded: Vec<String>,
}

impl PushState {
    async fn load(store: &Store) -> ferry_core::Result<Self> {
        Ok(match store.get_setting(PUSH_STATE_KEY).await? {
            Some(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("resetting unreadable GitHub push state: {e}");
                PushState::default()
            }),
            None => PushState::default(),
        })
    }

    async fn save(mut self, store: &Store) -> ferry_core::Result<()> {
        for list in [&mut self.payloads, &mut self.superseded] {
            if list.len() > PUSH_HISTORY {
                list.drain(..list.len() - PUSH_HISTORY);
            }
        }
        let raw = serde_json::to_string(&self)
            .map_err(|e| ferry_core::Error::internal(format!("encoding the GitHub push state: {e}")))?;
        store.set_setting(PUSH_STATE_KEY, &raw).await
    }
}

/// A commit sha from the payload (`None` for missing / all-zero shas).
fn payload_sha<'a>(payload: &'a Value, field: &str) -> Option<&'a str> {
    payload.get(field).and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty() && c.bytes().any(|b| b != b'0'))
}

/// Deploy every auto-deploy service tracking the pushed repository + branch.
async fn handle_push(st: &AppState, payload: &Value, body_hash: &str) -> ApiResult<PushOutcome> {
    let Some(branch) = payload.get("ref").and_then(Value::as_str).and_then(|r| r.strip_prefix("refs/heads/")) else {
        tracing::debug!("ignoring push to a non-branch ref");
        return Ok(PushOutcome::Deployed(Vec::new()));
    };
    if payload.get("deleted").and_then(Value::as_bool).unwrap_or(false) {
        tracing::debug!(branch, "ignoring push that deleted a branch");
        return Ok(PushOutcome::Deployed(Vec::new()));
    }
    let commit = payload_sha(payload, "after").map(str::to_string);
    if let Some(c) = &commit {
        validate::commit(c).map_err(|e| ApiError::bad_request(format!("invalid GitHub push payload: 'after': {e}")))?;
    }
    let before = payload_sha(payload, "before").filter(|c| validate::commit(c).is_ok());
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
    let flag = |k: &str| payload.get(k).and_then(Value::as_bool).unwrap_or(false);
    let (forced, created) = (flag("forced"), flag("created"));
    let branch_key = format!("{}|{branch}", urls[0]);

    let _guard = PUSH_LOCK.lock().await;
    let mut state = PushState::load(&st.store).await?;
    if state.payloads.iter().any(|h| h == body_hash) {
        return Ok(PushOutcome::Ignored("this push was already processed (duplicate or replayed delivery)".into()));
    }
    if let Some(c) = &commit
        && !forced
        && !created
        && state.superseded.contains(&format!("{branch_key}|{}", c.to_ascii_lowercase()))
    {
        return Ok(PushOutcome::Ignored(format!(
            "commit {c} was already superseded by a newer push to {branch} (replayed or out-of-order delivery)"
        )));
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

    state.payloads.push(body_hash.to_string());
    if let Some(b) = before {
        state.superseded.push(format!("{branch_key}|{}", b.to_ascii_lowercase()));
    }
    if let Err(e) = state.save(&st.store).await {
        // The deploys are queued; only the replay protection is degraded.
        tracing::warn!("saving the GitHub push state failed: {e}");
    }
    Ok(PushOutcome::Deployed(deploys))
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

    #[test]
    fn form_and_json_payloads() {
        let mut h = HeaderMap::new();
        assert_eq!(github_payload(&h, br#"{"a":1}"#).unwrap(), json!({"a": 1}));
        assert_eq!(github_payload(&h, b"payload=%7B%22a%22%3A2%7D").unwrap(), json!({"a": 2}));
        h.insert(header::CONTENT_TYPE, "application/x-www-form-urlencoded".parse().unwrap());
        assert_eq!(github_payload(&h, b"payload=%7B%22a%22%3A+3%7D").unwrap(), json!({"a": 3}));
        let err = github_payload(&h, b"other=1").unwrap_err();
        assert!(err.body.error.message.contains("application/json"), "{err}");
    }

    #[tokio::test]
    async fn push_state_is_bounded() {
        let store = Store::open_in_memory().await.unwrap();
        let mut state = PushState::load(&store).await.unwrap();
        state.payloads = (0..PUSH_HISTORY + 10).map(|i| i.to_string()).collect();
        state.save(&store).await.unwrap();
        let state = PushState::load(&store).await.unwrap();
        assert_eq!(state.payloads.len(), PUSH_HISTORY);
        assert_eq!(state.payloads[0], "10");
    }
}
