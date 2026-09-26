//! Webhooks: secret deploy hooks and GitHub push events. No bearer token.
//!
//! Responses never reveal more than the caller already proved it may know:
//! the deploy hook answers the same 401 for unknown services and wrong keys,
//! and returned deploys have credentials redacted ([`public_deploy`]).

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ferry_core::dto::ApiErrorBody;
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

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HookQuery {
    /// The service's deploy hook key (part of its `deploy_hook_path`).
    pub key: Option<String>,
}

/// `GET|POST /hooks/deploy/{service_id}?key=` — the path takes the service
/// **id** only (names are guessable); an unknown id and a wrong key get the
/// same 401, so the hook can't be used to discover services.
#[utoipa::path(
    post,
    path = "/hooks/deploy/{service_id}",
    operation_id = "deployHook",
    summary = "Deploy hook",
    description = "Queues a deploy of the service's configured source. The URL (`deploy_hook_path` of the service) is the secret: no token needed. Rotate it with `POST /api/v1/services/{id}/deploy-hook/rotate`.",
    tag = "hooks",
    params(
        ("service_id" = String, Path, description = "The service's **id** (names are not accepted: they are guessable)."),
        HookQuery,
    ),
    responses(
        (status = 202, description = "A deploy (trigger `deploy_hook`) was queued; credentials are redacted.", body = Deploy),
        (status = 400, description = "The service has no source to deploy.", body = ApiErrorBody),
        (status = 401, description = "Unknown service or wrong key (the same answer for both).", body = ApiErrorBody),
        (status = 409, description = "The service is suspended or being deleted.", body = ApiErrorBody),
    ),
)]
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

/// `GET /hooks/deploy/{service_id}?key=` — the same as `POST` (its own
/// handler only so the two methods get distinct OpenAPI operations).
#[utoipa::path(
    get,
    path = "/hooks/deploy/{service_id}",
    operation_id = "deployHookGet",
    summary = "Deploy hook (GET)",
    description = "Same as `POST`, for tools that can only send `GET` requests.",
    tag = "hooks",
    params(
        ("service_id" = String, Path, description = "The service's **id** (names are not accepted: they are guessable)."),
        HookQuery,
    ),
    responses(
        (status = 202, description = "A deploy (trigger `deploy_hook`) was queued; credentials are redacted.", body = Deploy),
        (status = 400, description = "The service has no source to deploy.", body = ApiErrorBody),
        (status = 401, description = "Unknown service or wrong key (the same answer for both).", body = ApiErrorBody),
        (status = 409, description = "The service is suspended or being deleted.", body = ApiErrorBody),
    ),
)]
pub async fn deploy_hook_get(
    state: State<AppState>,
    path: ApiPath<String>,
    query: ApiQuery<HookQuery>,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    deploy_hook(state, path, query).await
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
#[utoipa::path(
    post,
    path = "/hooks/github",
    tag = "hooks",
    operation_id = "githubWebhook",
    summary = "GitHub webhook",
    description = "Set it up in GitHub with the server's webhook secret (`github_webhook_secret`). A `push` deploys (trigger `webhook`, commit = `after`) every auto-deploy service whose repository and branch match; `ping` answers `{\"ok\": true}`; other events are ignored. Replayed deliveries (same body or delivery id) are ignored too.",
    params(
        ("X-Hub-Signature-256" = String, Header, description = "`sha256=<hex HMAC-SHA256 of the raw body>` with the webhook secret."),
        ("X-GitHub-Event" = String, Header, description = "`push`, `ping`, ..."),
        ("X-GitHub-Delivery" = Option<String>, Header, description = "Delivery id (used to ignore redeliveries)."),
    ),
    request_body(
        description = "The event payload: JSON, or form-encoded with the JSON in a `payload` field (GitHub's default content type). At most 25 MiB.",
        content(
            (Object = "application/json"),
            ("application/x-www-form-urlencoded"),
        ),
    ),
    responses(
        (status = 200, description = "Processed (see which fields are set).", body = crate::openapi::GithubHookResponse),
        (status = 400, description = "Invalid payload.", body = ApiErrorBody),
        (status = 401, description = "Missing or invalid signature.", body = ApiErrorBody),
        (status = 404, description = "GitHub webhooks are not enabled (no webhook secret configured).", body = ApiErrorBody),
        (status = 413, description = "The payload exceeds 25 MiB.", body = ApiErrorBody),
    ),
)]
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
            // GitHub's delivery ids are GUIDs; anything unusual isn't used for dedupe.
            let delivery = header_str(&headers, "x-github-delivery")
                .map(str::trim)
                .filter(|d| d.len() <= 128)
                .unwrap_or("")
                .to_string();
            // Finish (deploys + replay bookkeeping) even if GitHub hangs up.
            let body_hash = hex::encode(Sha256::digest(&body));
            let seen = Delivery { id: delivery.clone(), body_hash };
            let outcome = crate::locks::detached(async move { handle_push(&st, &payload, &seen).await }).await?;
            let delivery = if delivery.is_empty() { "-".to_string() } else { delivery };
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

/// Identity of one delivery, for deduplication.
struct Delivery {
    /// `X-GitHub-Delivery` (empty when absent). GitHub keeps it on redeliveries.
    id: String,
    /// SHA-256 of the raw (signed) body.
    body_hash: String,
}

/// What protects `/hooks/github` against replays and duplicate deliveries.
/// The HMAC covers only the body (no timestamp) and `X-GitHub-Delivery`
/// isn't signed, so a push is ignored when its exact signed body
/// (`payloads`) or its delivery id (`deliveries`) was already processed.
///
/// Commits are deliberately *not* compared with earlier pushes: pushing a
/// commit the branch was at before (a fast-forward re-push after a
/// force-push rollback, a revert of a revert...) is a legitimate deploy.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PushState {
    #[serde(default)]
    payloads: Vec<String>,
    #[serde(default)]
    deliveries: Vec<String>,
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
        for list in [&mut self.payloads, &mut self.deliveries] {
            if list.len() > PUSH_HISTORY {
                list.drain(..list.len() - PUSH_HISTORY);
            }
        }
        let raw = serde_json::to_string(&self)
            .map_err(|e| ferry_core::Error::internal(format!("encoding the GitHub push state: {e}")))?;
        store.set_setting(PUSH_STATE_KEY, &raw).await
    }

    fn seen(&self, d: &Delivery) -> bool {
        self.payloads.contains(&d.body_hash) || (!d.id.is_empty() && self.deliveries.contains(&d.id))
    }

    fn record(&mut self, d: &Delivery) {
        self.payloads.push(d.body_hash.clone());
        if !d.id.is_empty() {
            self.deliveries.push(d.id.clone());
        }
    }
}

/// A commit sha from the payload (`None` for missing / all-zero shas).
fn payload_sha<'a>(payload: &'a Value, field: &str) -> Option<&'a str> {
    payload.get(field).and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty() && c.bytes().any(|b| b != b'0'))
}

/// Deploy every auto-deploy service tracking the pushed repository + branch.
async fn handle_push(st: &AppState, payload: &Value, delivery: &Delivery) -> ApiResult<PushOutcome> {
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

    let _guard = PUSH_LOCK.lock().await;
    let mut state = PushState::load(&st.store).await?;
    if state.seen(delivery) {
        return Ok(PushOutcome::Ignored("this push was already processed (duplicate or replayed delivery)".into()));
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

    state.record(delivery);
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
        state.deliveries = (0..PUSH_HISTORY + 1).map(|i| format!("d{i}")).collect();
        state.save(&store).await.unwrap();
        let state = PushState::load(&store).await.unwrap();
        assert_eq!(state.payloads.len(), PUSH_HISTORY);
        assert_eq!(state.payloads[0], "10");
        assert_eq!((state.deliveries.len(), state.deliveries[0].as_str()), (PUSH_HISTORY, "d1"));
    }

    #[tokio::test]
    async fn older_push_state_is_still_readable() {
        // Written by versions that also tracked superseded commits.
        let store = Store::open_in_memory().await.unwrap();
        store.set_setting(PUSH_STATE_KEY, r#"{"payloads":["h1"],"superseded":["x|main|abc"]}"#).await.unwrap();
        let state = PushState::load(&store).await.unwrap();
        assert!(state.seen(&Delivery { id: String::new(), body_hash: "h1".into() }));
        assert!(!state.seen(&Delivery { id: String::new(), body_hash: "h2".into() }));
        let mut state = state;
        state.record(&Delivery { id: "d-9".into(), body_hash: "h2".into() });
        assert!(state.seen(&Delivery { id: "d-9".into(), body_hash: "other".into() }));
        assert!(!state.seen(&Delivery { id: "d-10".into(), body_hash: "other".into() }));
    }
}
