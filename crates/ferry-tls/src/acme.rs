//! ACME protocol: account management and the HTTP-01 order flow
//! (instant-acme 0.8).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount, NewOrder, Order,
    OrderStatus, Problem,
};
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, sleep};
use tracing::{debug, info, warn};

use crate::AcmeClientFactory;
use crate::certs;
use crate::storage;

/// First delay between two order polls.
const POLL_INITIAL: Duration = Duration::from_millis(500);
/// Longest delay between two order polls.
const POLL_MAX: Duration = Duration::from_secs(5);
/// How long the CA gets to validate the challenge, and again to issue.
const POLL_TIMEOUT: Duration = Duration::from_secs(120);

/// Why obtaining a certificate failed.
#[derive(Debug, Clone)]
pub(crate) struct IssueError {
    pub message: String,
    /// The CA no longer knows our account: it must be recreated.
    pub account_gone: bool,
}

impl IssueError {
    pub(crate) fn other(message: impl Into<String>) -> Self {
        IssueError { message: message.into(), account_gone: false }
    }

    pub(crate) fn acme(context: impl std::fmt::Display, err: instant_acme::Error) -> Self {
        let account_gone = matches!(&err, instant_acme::Error::Api(p) if problem_means_account_gone(p));
        IssueError { message: format!("{context}: {err}"), account_gone }
    }

    fn problem(context: &str, problem: &Problem) -> Self {
        IssueError { message: format!("{context}: {problem}"), account_gone: problem_means_account_gone(problem) }
    }
}

impl std::fmt::Display for IssueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<ferry_core::Error> for IssueError {
    fn from(e: ferry_core::Error) -> Self {
        IssueError::other(e.to_string())
    }
}

fn problem_means_account_gone(p: &Problem) -> bool {
    match p.r#type.as_deref() {
        Some("urn:ietf:params:acme:error:accountDoesNotExist") => true,
        Some("urn:ietf:params:acme:error:unauthorized") => {
            p.detail.as_deref().is_some_and(|d| d.to_ascii_lowercase().contains("deactivated"))
        }
        _ => false,
    }
}

/// `mailto:` contact list for the account (empty email → no contact).
pub(crate) fn contacts_for(email: &str) -> Vec<String> {
    let email = email.trim();
    if email.is_empty() {
        Vec::new()
    } else if email.starts_with("mailto:") {
        vec![email.to_string()]
    } else {
        vec![format!("mailto:{email}")]
    }
}

/// Contents of `accounts/<name>.json`.
#[derive(Serialize, Deserialize)]
struct StoredAccount {
    directory_url: String,
    #[serde(default)]
    contact: Vec<String>,
    created_at: DateTime<Utc>,
    /// instant-acme's `AccountCredentials` (kept as JSON: that type is not `Clone`).
    credentials: serde_json::Value,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, IssueError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| IssueError::other(format!("background task failed: {e}")))
}

async fn read_optional(path: PathBuf) -> Result<Option<Vec<u8>>, IssueError> {
    let display = path.display().to_string();
    blocking(move || match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    })
    .await?
    .map_err(|e| IssueError::other(format!("reading {display}: {e}")))
}

async fn save_account(path: PathBuf, stored: &StoredAccount) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(stored).map_err(|e| e.to_string())?;
    let display = path.display().to_string();
    blocking(move || {
        if let Some(dir) = path.parent() {
            storage::create_private_dir(dir)?;
        }
        storage::write_atomic(&path, &json, true)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("writing {display}: {e}"))
}

/// Rename an account file out of the way (kept for forensics, never reused).
pub(crate) async fn retire_account_file(certs_dir: &Path, directory_url: &str) {
    let path = storage::account_path(certs_dir, directory_url);
    let backup = path.with_extension(format!("json.retired-{}", Utc::now().format("%Y%m%dT%H%M%S")));
    let (from, to) = (path.clone(), backup.clone());
    match blocking(move || std::fs::rename(from, to)).await {
        Ok(Ok(())) => warn!("moved ACME account file {} to {}", path.display(), backup.display()),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(Err(e)) => warn!("cannot move ACME account file {} aside: {e}", path.display()),
        Err(e) => warn!("cannot move ACME account file {} aside: {e}", path.display()),
    }
}

/// Restore the ACME account stored for `directory_url`, or register a new
/// one (terms of service agreed, `mailto:<email>` contact) and store it.
pub(crate) async fn load_or_create_account(
    client: &AcmeClientFactory,
    certs_dir: &Path,
    directory_url: &str,
    email: &str,
) -> Result<Account, IssueError> {
    let path = storage::account_path(certs_dir, directory_url);
    let contact = contacts_for(email);
    if let Some(bytes) = read_optional(path.clone()).await? {
        match serde_json::from_slice::<StoredAccount>(&bytes) {
            Ok(stored) if stored.directory_url == directory_url => {
                let credentials: AccountCredentials = serde_json::from_value(stored.credentials.clone())
                    .map_err(|e| IssueError::other(format!("invalid credentials in {}: {e}", path.display())))?;
                let builder = client().map_err(|e| IssueError::acme("preparing the ACME HTTP client", e))?;
                let account = builder
                    .from_credentials(credentials)
                    .await
                    .map_err(|e| IssueError::acme(format_args!("restoring the ACME account at {directory_url}"), e))?;
                debug!("using ACME account {} from {}", account.id(), path.display());
                if stored.contact != contact {
                    update_contact(&account, path, stored, contact).await;
                }
                return Ok(account);
            }
            Ok(stored) => warn!(
                "{} belongs to ACME directory {}, not {directory_url}; registering a new account",
                path.display(),
                stored.directory_url
            ),
            Err(e) => warn!("{} is not a valid ACME account file ({e}); registering a new account", path.display()),
        }
        retire_account_file(certs_dir, directory_url).await;
    }

    let contact_refs: Vec<&str> = contact.iter().map(String::as_str).collect();
    let builder = client().map_err(|e| IssueError::acme("preparing the ACME HTTP client", e))?;
    let (account, credentials) = builder
        .create(
            &NewAccount { contact: &contact_refs, terms_of_service_agreed: true, only_return_existing: false },
            directory_url.to_string(),
            None,
        )
        .await
        .map_err(|e| IssueError::acme(format_args!("registering an ACME account at {directory_url}"), e))?;
    info!("registered ACME account {} at {directory_url}", account.id());
    match serde_json::to_value(&credentials) {
        Ok(credentials) => {
            let stored = StoredAccount {
                directory_url: directory_url.to_string(),
                contact,
                created_at: Utc::now(),
                credentials,
            };
            if let Err(e) = save_account(path, &stored).await {
                // Keep going with the in-memory account; the next restart registers another one.
                warn!("cannot save the ACME account: {e}");
            }
        }
        Err(e) => warn!("cannot serialize the ACME account credentials: {e}"),
    }
    Ok(account)
}

/// Best effort: push a changed contact email to the CA and store it.
async fn update_contact(account: &Account, path: PathBuf, mut stored: StoredAccount, contact: Vec<String>) {
    let refs: Vec<&str> = contact.iter().map(String::as_str).collect();
    match account.update_contacts(&refs).await {
        Ok(()) => {
            info!(
                "updated the ACME account contact to {}",
                if contact.is_empty() { "(none)".into() } else { contact.join(", ") }
            );
            stored.contact = contact;
            if let Err(e) = save_account(path, &stored).await {
                warn!("cannot save the ACME account: {e}");
            }
        }
        Err(e) => warn!("cannot update the ACME account contact: {e}"),
    }
}

/// Published HTTP-01 answers; removes them from the shared map when dropped,
/// whatever way the order ends (success, error, timeout or cancellation).
pub(crate) struct PublishedChallenges<'a> {
    map: &'a RwLock<HashMap<String, String>>,
    tokens: Vec<String>,
}

impl<'a> PublishedChallenges<'a> {
    pub(crate) fn new(map: &'a RwLock<HashMap<String, String>>) -> Self {
        PublishedChallenges { map, tokens: Vec::new() }
    }

    pub(crate) fn publish(&mut self, token: String, key_authorization: String) {
        self.map.write().unwrap_or_else(PoisonError::into_inner).insert(token.clone(), key_authorization);
        self.tokens.push(token);
    }
}

impl Drop for PublishedChallenges<'_> {
    fn drop(&mut self) {
        if self.tokens.is_empty() {
            return;
        }
        let mut map = self.map.write().unwrap_or_else(PoisonError::into_inner);
        for token in self.tokens.drain(..) {
            map.remove(&token);
        }
    }
}

/// PEM chain + private key of a freshly issued certificate.
pub(crate) struct IssuedPem {
    pub chain_pem: String,
    pub key_pem: String,
}

/// Run a complete HTTP-01 order for a single `host`.
pub(crate) async fn order_certificate(
    account: &Account,
    host: &str,
    challenges: &RwLock<HashMap<String, String>>,
) -> Result<IssuedPem, IssueError> {
    let identifiers = [Identifier::Dns(host.to_string())];
    let mut order =
        account.new_order(&NewOrder::new(&identifiers)).await.map_err(|e| IssueError::acme("creating the order", e))?;
    debug!("ACME order for {host}: {}", order.url());

    let published = {
        let mut published = PublishedChallenges::new(challenges);
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result.map_err(|e| IssueError::acme("fetching the authorization", e))?;
            match authz.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                status => return Err(IssueError::other(format!("authorization is {status:?}"))),
            }
            let mut challenge = authz
                .challenge(ChallengeType::Http01)
                .ok_or_else(|| IssueError::other("the CA offered no http-01 challenge"))?;
            let key_authorization = challenge.key_authorization().as_str().to_string();
            published.publish(challenge.token.clone(), key_authorization);
            challenge.set_ready().await.map_err(|e| IssueError::acme("answering the http-01 challenge", e))?;
        }
        published
    };

    wait_until_ready(&mut order).await?;
    // Validation is over: stop answering the challenge.
    drop(published);

    let key = certs::generate_key()?;
    let csr = certs::csr_der(host, &key)?;
    order.finalize_csr(&csr).await.map_err(|e| IssueError::acme("finalizing the order", e))?;
    let chain_pem = wait_for_certificate(&mut order).await?;
    Ok(IssuedPem { chain_pem, key_pem: key.serialize_pem() })
}

/// `(status, error)` of the last known order state.
fn snapshot(order: &mut Order) -> (OrderStatus, Option<Problem>) {
    let state = order.state();
    (state.status, state.error.clone())
}

/// Sleep before the next poll, or fail when the deadline would be exceeded.
async fn next_poll(delay: &mut Duration, deadline: Instant, what: &str) -> Result<(), IssueError> {
    if Instant::now() + *delay > deadline {
        return Err(IssueError::other(format!("timed out after {}s {what}", POLL_TIMEOUT.as_secs())));
    }
    sleep(*delay).await;
    *delay = (*delay * 3 / 2).min(POLL_MAX);
    Ok(())
}

/// Poll the order until the CA validated every authorization.
async fn wait_until_ready(order: &mut Order) -> Result<(), IssueError> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    let mut delay = POLL_INITIAL;
    loop {
        let (status, error) = snapshot(order);
        if let Some(problem) = error {
            return Err(IssueError::problem("the order failed", &problem));
        }
        match status {
            OrderStatus::Ready => return Ok(()),
            OrderStatus::Pending => {}
            OrderStatus::Invalid => {
                let reasons = authorization_errors(order).await;
                return Err(IssueError::other(if reasons.is_empty() {
                    "the CA could not validate the http-01 challenge".to_string()
                } else {
                    format!("the CA could not validate the http-01 challenge: {}", reasons.join("; "))
                }));
            }
            other => return Err(IssueError::other(format!("unexpected order status {other:?} before finalization"))),
        }
        next_poll(&mut delay, deadline, "waiting for the CA to validate the http-01 challenge").await?;
        order.refresh().await.map_err(|e| IssueError::acme("polling the order", e))?;
    }
}

/// Poll a finalized order until the certificate is issued, then download it.
async fn wait_for_certificate(order: &mut Order) -> Result<String, IssueError> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    let mut delay = POLL_INITIAL;
    loop {
        let (status, error) = snapshot(order);
        if let Some(problem) = error {
            return Err(IssueError::problem("the order failed", &problem));
        }
        match status {
            OrderStatus::Valid => {
                return match order.certificate().await {
                    Ok(Some(pem)) => Ok(pem),
                    Ok(None) => Err(IssueError::other("the CA returned no certificate")),
                    Err(e) => Err(IssueError::acme("downloading the certificate", e)),
                };
            }
            // `ready` right after finalization: the CA hasn't picked it up yet.
            OrderStatus::Processing | OrderStatus::Ready => {}
            other => return Err(IssueError::other(format!("order became {other:?} during finalization"))),
        }
        next_poll(&mut delay, deadline, "waiting for the certificate").await?;
        order.refresh().await.map_err(|e| IssueError::acme("polling the order", e))?;
    }
}

/// Best effort: collect challenge errors of an invalid order.
async fn authorization_errors(order: &mut Order) -> Vec<String> {
    let mut reasons = Vec::new();
    let mut authorizations = order.authorizations();
    while let Some(result) = authorizations.next().await {
        let Ok(mut authz) = result else { break };
        let Ok(state) = authz.refresh().await else { continue };
        reasons.extend(state.challenges.iter().filter_map(|c| c.error.as_ref()).map(ToString::to_string));
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contacts() {
        assert!(contacts_for("  ").is_empty());
        assert_eq!(contacts_for("ops@example.com"), vec!["mailto:ops@example.com"]);
        assert_eq!(contacts_for("mailto:ops@example.com"), vec!["mailto:ops@example.com"]);
    }

    #[test]
    fn challenges_are_removed_on_drop() {
        let map = RwLock::new(HashMap::new());
        map.write().unwrap().insert("other".to_string(), "keep".to_string());
        {
            let mut published = PublishedChallenges::new(&map);
            published.publish("t1".into(), "t1.thumb".into());
            published.publish("t2".into(), "t2.thumb".into());
            assert_eq!(map.read().unwrap().get("t1").map(String::as_str), Some("t1.thumb"));
            assert_eq!(map.read().unwrap().len(), 3);
        }
        let map = map.into_inner().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("other"));
    }

    #[test]
    fn account_gone_detection() {
        let problem = |t: &str, d: &str| -> Problem {
            serde_json::from_value(serde_json::json!({"type": t, "detail": d})).unwrap()
        };
        assert!(problem_means_account_gone(&problem("urn:ietf:params:acme:error:accountDoesNotExist", "")));
        assert!(problem_means_account_gone(&problem(
            "urn:ietf:params:acme:error:unauthorized",
            "Account is not valid, has status \"deactivated\""
        )));
        assert!(!problem_means_account_gone(&problem("urn:ietf:params:acme:error:rateLimited", "too many")));
        let e = IssueError::acme("creating the order", instant_acme::Error::Str("boom"));
        assert!(!e.account_gone);
        assert_eq!(e.to_string(), "creating the order: missing data: boom");
    }

    #[tokio::test]
    async fn stored_account_with_other_directory_is_retired() {
        let tmp = tempfile::tempdir().unwrap();
        let url = "https://acme.invalid/dir";
        let path = storage::account_path(tmp.path(), url);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let client: AcmeClientFactory = std::sync::Arc::new(move || {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(instant_acme::Error::Str("offline"))
        });
        let err = load_or_create_account(&client, tmp.path(), url, "a@b.c").await.err().unwrap();
        assert!(err.message.contains("offline"), "{err}");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // the corrupt file was moved aside, not deleted
        assert!(!path.exists());
        let retired = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(retired, 1);
    }
}
