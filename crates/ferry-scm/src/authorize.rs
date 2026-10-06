//! Authorizing an account in the browser, step by step.
//!
//! [`start`] answers with the provider's page to send the browser to; the
//! provider sends it back to the dashboard's `/git/callback` page, which
//! hands the query parameters to [`callback`]; that answers with the next
//! page, or with the connection once the account is connected.
//!
//! * **GitHub**: register a GitHub App for this server from a manifest
//!   (`?code=`), then install it on the account (`?installation_id=`). A
//!   server GitHub can reach registers the app with a webhook, so pushes to
//!   the account's repositories are delivered to it.
//! * **GitLab**: authorize the OAuth application created for this server
//!   (`?code=`).
//!
//! Every page carries a `state` this module made up and remembers (in
//! memory, for an hour, usable once): an answer that doesn't bring one back
//! was not asked for here.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use ferry_core::dto::{AuthorizeGit, GitCallback};
use ferry_core::{GitAuth, GitConnection, GitProvider, GithubApp, GithubInstallation, Store, ids, validate};

use crate::provider::{AppRegistration, Installation, OAuthTokens, Provider, ProviderError};
use crate::{access, github_app, oauth};

/// How long the browser has to come back (GitHub keeps a manifest code for
/// an hour).
const FLOW_TTL: Duration = Duration::from_secs(60 * 60);
/// More unfinished authorizations than this: the oldest are forgotten.
const MAX_FLOWS: usize = 256;

/// Why an authorization step failed.
#[derive(Debug)]
pub enum FlowError {
    /// The request is wrong, or about something that doesn't exist.
    Core(ferry_core::Error),
    /// GitLab needs the id and secret of an OAuth application first.
    ApplicationRequired(String),
    /// The provider refused or failed.
    Provider(ProviderError),
}

impl From<ferry_core::Error> for FlowError {
    fn from(e: ferry_core::Error) -> Self {
        FlowError::Core(e)
    }
}

impl From<ProviderError> for FlowError {
    fn from(e: ProviderError) -> Self {
        FlowError::Provider(e)
    }
}

fn invalid(message: impl Into<String>) -> FlowError {
    FlowError::Core(ferry_core::Error::invalid(message))
}

/// What the browser does next.
#[derive(Debug)]
pub enum Step {
    /// Go to this page of the provider.
    Redirect(String),
    /// Post this form to the provider.
    Form { url: String, fields: BTreeMap<String, String> },
    /// Nothing: the account is connected.
    Connected(Box<GitConnection>),
}

impl Step {
    fn connected(connection: GitConnection) -> Self {
        Step::Connected(Box::new(connection))
    }
}

/// What an expected answer of the provider is about.
#[derive(Debug, Clone)]
enum Pending {
    /// GitHub registers the app: it answers with a manifest `code`.
    /// `webhook`: the manifest asked for a webhook.
    GithubApp { base_url: String, webhook: bool },
    /// GitHub installs this connection's app: it answers with an
    /// `installation_id`.
    GithubInstall { connection_id: String },
    /// The provider asks the account to authorize this connection's OAuth
    /// application: it answers with a `code`.
    Oauth { connection_id: String, redirect_uri: String },
}

impl Pending {
    fn provider(&self) -> GitProvider {
        match self {
            Pending::GithubApp { .. } | Pending::GithubInstall { .. } => GitProvider::Github,
            Pending::Oauth { .. } => GitProvider::Gitlab,
        }
    }
}

/// The answers the providers still owe, by `state`.
#[derive(Default)]
struct Flows(Mutex<HashMap<String, (Pending, Instant)>>);

impl Flows {
    /// Remember what the provider is asked, under a new `state`.
    fn remember(&self, pending: Pending) -> String {
        let state = ids::random_secret(48);
        let now = Instant::now();
        let mut flows = self.0.lock().unwrap_or_else(|e| e.into_inner());
        flows.retain(|_, (_, at)| now.duration_since(*at) < FLOW_TTL);
        while flows.len() >= MAX_FLOWS {
            let Some(oldest) = flows.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone()) else { break };
            flows.remove(&oldest);
        }
        flows.insert(state.clone(), (pending, now));
        state
    }

    /// What `state` was made up for, once.
    fn take(&self, state: &str) -> Option<Pending> {
        let (pending, at) = self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(state)?;
        (at.elapsed() < FLOW_TTL).then_some(pending)
    }
}

static FLOWS: LazyLock<Flows> = LazyLock::new(Flows::default);

/// One connection is found-then-written at a time (one row per account).
static WRITES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn remember(pending: Pending) -> String {
    FLOWS.remember(pending)
}

fn take(state: &str) -> Option<Pending> {
    FLOWS.take(state)
}

/// The provider instance to talk to: the given web URL (a missing scheme
/// means https), else the provider's public one.
pub fn instance_url(provider: GitProvider, base_url: Option<&str>) -> ferry_core::Result<String> {
    match base_url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) if u.contains("://") => validate::git_base_url(u),
        Some(u) => validate::git_base_url(&format!("https://{u}")),
        None => Ok(provider.default_base_url().to_string()),
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// Start authorizing an account (or resume / repeat the authorization of
/// `connection_id`).
///
/// `webhook_url` is where GitHub can deliver the pushes of the account's
/// repositories (this server's GitHub webhook endpoint, at an address on
/// the internet): a GitHub App registered now gets it as its webhook.
/// `None` for a server GitHub can't reach.
pub async fn start(store: &Store, req: &AuthorizeGit, webhook_url: Option<&str>) -> Result<Step, FlowError> {
    let redirect_uri = validate::git_redirect_uri(&req.redirect_uri)?;
    if let Some(id) = non_empty(&req.connection_id) {
        let connection = store.require_git_connection(id).await?;
        return match connection.auth {
            GitAuth::GithubApp => install(store, connection).await,
            GitAuth::Oauth => {
                let connection = match application_of(req)? {
                    Some((client_id, client_secret)) => {
                        replace_application(store, connection, client_id, client_secret).await?
                    }
                    None => connection,
                };
                authorize_application(&connection, redirect_uri)
            }
            GitAuth::Token => Err(invalid(format!(
                "the {} is connected with an access token: give it a new token, or authorize {} without connection_id",
                connection.describe(),
                connection.provider.label()
            ))),
        };
    }
    let provider = req.provider.ok_or_else(|| invalid("provider is required: github or gitlab"))?;
    let base_url = instance_url(provider, req.base_url.as_deref())?;
    match provider {
        GitProvider::Github => {
            let organization = non_empty(&req.organization).map(validate::git_organization).transpose()?;
            let state = remember(Pending::GithubApp { base_url: base_url.clone(), webhook: webhook_url.is_some() });
            let manifest = github_app::manifest(redirect_uri, webhook_url);
            let fields = BTreeMap::from([("manifest".to_string(), manifest.to_string())]);
            Ok(Step::Form { url: github_app::registration_url(&base_url, organization, &state), fields })
        }
        GitProvider::Gitlab => {
            let connection = application(store, provider, &base_url, req).await?;
            authorize_application(&connection, redirect_uri)
        }
    }
}

/// The OAuth application of a request, when it brings one.
fn application_of(req: &AuthorizeGit) -> Result<Option<(&str, &str)>, FlowError> {
    match (non_empty(&req.client_id), non_empty(&req.client_secret)) {
        (Some(id), Some(secret)) => Ok(Some((
            validate::git_client_credential("application id", id)?,
            validate::git_client_credential("application secret", secret)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(invalid("client_id and client_secret go together")),
    }
}

async fn replace_application(
    store: &Store,
    mut connection: GitConnection,
    client_id: &str,
    client_secret: &str,
) -> Result<GitConnection, FlowError> {
    connection.client_id = Some(client_id.to_string());
    connection.client_secret = Some(client_secret.to_string());
    Ok(store.update_git_connection(&connection).await?)
}

/// The connection that holds the OAuth application to authorize on this
/// instance: the one the request brings (kept as a pending connection until
/// an account authorizes it), else the one given before.
async fn application(
    store: &Store,
    provider: GitProvider,
    base_url: &str,
    req: &AuthorizeGit,
) -> Result<GitConnection, FlowError> {
    let _writing = WRITES.lock().await;
    let pending = store.find_git_connection(provider, base_url, "").await?;
    match (application_of(req)?, pending) {
        (Some((client_id, client_secret)), Some(pending)) => {
            replace_application(store, pending, client_id, client_secret).await
        }
        (Some((client_id, client_secret)), None) => {
            let connection = GitConnection::oauth_application(provider, base_url, client_id, client_secret);
            store.create_git_connection(&connection).await?;
            Ok(connection)
        }
        (None, Some(pending)) if pending.client_id.is_some() && pending.client_secret.is_some() => Ok(pending),
        (None, _) => {
            // The application an account of this instance already authorized.
            let known = store.list_git_connections().await?.into_iter().find(|c| {
                c.provider == provider
                    && c.base_url == base_url
                    && c.auth == GitAuth::Oauth
                    && c.client_id.is_some()
                    && c.client_secret.is_some()
            });
            known.ok_or_else(|| {
                FlowError::ApplicationRequired(format!(
                    "{label} needs an OAuth application for this server: create one on {base_url} (redirect URI: \
                     the redirect_uri of this request; scopes: {scopes}) and give its client_id and client_secret",
                    label = provider.label(),
                    scopes = oauth::GITLAB_SCOPES.join(", "),
                ))
            })
        }
    }
}

/// Send the browser to the page where the account authorizes the
/// connection's OAuth application.
fn authorize_application(connection: &GitConnection, redirect_uri: &str) -> Result<Step, FlowError> {
    let client_id = connection.client_id.as_deref().ok_or_else(|| {
        FlowError::ApplicationRequired(format!("the {} has no OAuth application", connection.describe()))
    })?;
    let state =
        remember(Pending::Oauth { connection_id: connection.id.clone(), redirect_uri: redirect_uri.to_string() });
    Ok(Step::Redirect(oauth::authorize_url(&connection.base_url, client_id, redirect_uri, &state)?))
}

fn app_of(connection: &GitConnection) -> Result<&GithubApp, FlowError> {
    connection.app.as_ref().ok_or_else(|| {
        FlowError::Core(ferry_core::Error::internal(format!("the {} has no GitHub App", connection.describe())))
    })
}

/// The installation a connection is about, among its app's: the one it
/// already has, else the one on its account, else the only one.
fn pick(installations: Vec<Installation>, connection: &GitConnection) -> Option<Installation> {
    let known = connection.installation.as_ref().map(|i| i.id);
    let score =
        |i: &Installation| (Some(i.id) == known, i.account.eq_ignore_ascii_case(&connection.account), !i.suspended);
    let single = installations.len() == 1;
    installations.into_iter().max_by_key(|i| score(i)).filter(|i| single || score(i).0 || score(i).1)
}

/// The next step for a GitHub App: it is already installed (the browser
/// never came back from the installation, or the connection is checked
/// again) and the connection is ready, or GitHub's installation page.
async fn install(store: &Store, connection: GitConnection) -> Result<Step, FlowError> {
    let app = app_of(&connection)?;
    let jwt = github_app::jwt(app, Utc::now())?;
    let installations = Provider::new(GitProvider::Github, &connection.base_url, &jwt).app_installations().await?;
    if let Some(found) = pick(installations, &connection) {
        return Ok(Step::connected(adopt(store, connection, found).await?));
    }
    let state = remember(Pending::GithubInstall { connection_id: connection.id.clone() });
    Ok(Step::Redirect(github_app::install_url(app, &state)))
}

/// Record where the connection's app is installed: the connection is ready.
async fn adopt(
    store: &Store,
    mut connection: GitConnection,
    installation: Installation,
) -> Result<GitConnection, FlowError> {
    if installation.suspended {
        return Err(FlowError::Provider(ProviderError::Forbidden(format!(
            "the GitHub App '{}' is suspended on '{}': unsuspend it in GitHub's settings",
            app_of(&connection)?.slug,
            installation.account
        ))));
    }
    let _writing = WRITES.lock().await;
    if !installation.account.is_empty() {
        if !installation.account.eq_ignore_ascii_case(&connection.account) {
            // Installed on another account than the one it was registered
            // for: that account is what the connection reads. What
            // connected it before is superseded.
            let previous =
                store.find_git_connection(GitProvider::Github, &connection.base_url, &installation.account).await?;
            if let Some(previous) = previous.filter(|p| p.id != connection.id) {
                store.delete_git_connection(&previous.id).await?;
                access::forget(&previous.id);
            }
        }
        // As GitHub spells it.
        connection.account = installation.account.clone();
    }
    connection.installation = Some(GithubInstallation {
        id: installation.id,
        url: installation.url,
        repository_selection: installation.repository_selection,
    });
    access::forget(&connection.id);
    let saved = store.update_git_connection(&connection).await?;
    tracing::info!(connection = %saved.id, installation = installation.id, "the GitHub App is installed on the {}", saved.describe());
    Ok(saved)
}

/// The secret that signs the deliveries of a new app's webhook: the one
/// GitHub made up for it, else one given to the app now. `None` for an app
/// registered without a webhook, and when no secret can be set: the account
/// is connected all the same, but its pushes are not accepted here.
async fn webhook_secret(base_url: &str, registration: &AppRegistration, webhook: bool) -> Option<String> {
    if !webhook {
        return None;
    }
    if let Some(secret) = &registration.webhook_secret {
        return Some(secret.clone());
    }
    let app = GithubApp {
        id: registration.id,
        slug: registration.slug.clone(),
        url: registration.url.clone(),
        private_key: registration.private_key.clone(),
        webhook_secret: None,
    };
    let secret = ids::random_secret(48);
    let set = match github_app::jwt(&app, Utc::now()) {
        Ok(jwt) => Provider::new(GitProvider::Github, base_url, &jwt).set_app_webhook_secret(&secret).await,
        Err(e) => Err(e),
    };
    match set {
        Ok(()) => Some(secret),
        Err(e) => {
            tracing::warn!(
                app = %app.slug,
                "GitHub gave the app's webhook no secret and took none: its pushes will not deploy anything ({e})"
            );
            None
        }
    }
}

/// Keep the app GitHub just registered: as the connection of its owner,
/// replacing whatever connected that account before.
async fn save_app(store: &Store, base_url: &str, registration: AppRegistration) -> Result<GitConnection, FlowError> {
    let AppRegistration { id, slug, url, client_id, client_secret, private_key, webhook_secret, owner } = registration;
    if owner.trim().is_empty() {
        return Err(FlowError::Provider(ProviderError::Unavailable(
            "GitHub didn't say which account the app belongs to".into(),
        )));
    }
    let app = GithubApp { id, slug, url, private_key, webhook_secret };
    let _writing = WRITES.lock().await;
    let saved = match store.find_git_connection(GitProvider::Github, base_url, &owner).await? {
        Some(mut existing) => {
            existing.auth = GitAuth::GithubApp;
            existing.account = owner;
            existing.token = String::new();
            existing.refresh_token = None;
            existing.scopes = Vec::new();
            existing.token_expires_at = None;
            existing.client_id = Some(client_id);
            existing.client_secret = Some(client_secret);
            existing.app = Some(app);
            existing.installation = None;
            access::forget(&existing.id);
            store.update_git_connection(&existing).await?
        }
        None => {
            let connection = GitConnection::github_app(base_url, owner, app, client_id, client_secret);
            store.create_git_connection(&connection).await?;
            connection
        }
    };
    tracing::info!(connection = %saved.id, "registered a GitHub App for the {}", saved.describe());
    Ok(saved)
}

/// Keep the tokens an account authorized `application` with: on that
/// account's connection (one per account), which is `application` itself
/// while it waits for its first account.
async fn save_authorization(
    store: &Store,
    application: GitConnection,
    login: String,
    name: Option<String>,
    tokens: OAuthTokens,
) -> Result<GitConnection, FlowError> {
    let fill = |c: &mut GitConnection| {
        c.auth = GitAuth::Oauth;
        c.account = login.clone();
        c.account_name = name.clone();
        c.token = tokens.access_token.clone();
        c.refresh_token = tokens.refresh_token.clone();
        c.token_expires_at = tokens.expires_at;
        c.scopes = tokens.scopes.clone();
        c.client_id = application.client_id.clone();
        c.client_secret = application.client_secret.clone();
        c.app = None;
        c.installation = None;
    };
    let _writing = WRITES.lock().await;
    let existing = store.find_git_connection(application.provider, &application.base_url, &login).await?;
    let saved = match existing {
        Some(mut target) => {
            fill(&mut target);
            let saved = store.update_git_connection(&target).await?;
            // The application's own (pending) row has served.
            if application.id != saved.id && application.account.is_empty() {
                let _ = store.delete_git_connection(&application.id).await;
            }
            saved
        }
        None if application.account.is_empty() => {
            let mut target = application.clone();
            fill(&mut target);
            store.update_git_connection(&target).await?
        }
        // Another account authorizes an application one already uses.
        None => {
            let mut target = GitConnection::new(application.provider, &application.base_url, login.clone(), "");
            fill(&mut target);
            store.create_git_connection(&target).await?;
            target
        }
    };
    access::forget(&saved.id);
    tracing::info!(connection = %saved.id, "the {} authorized this server", saved.describe());
    Ok(saved)
}

/// GitHub sent the browser back without a `state`: an installation was
/// made or changed from GitHub's own pages. Read it again, on the
/// connection whose app it is an installation of.
async fn installation_changed(store: &Store, installation_id: u64) -> Result<Step, FlowError> {
    let mut apps: Vec<GitConnection> =
        store.list_git_connections().await?.into_iter().filter(|c| c.auth == GitAuth::GithubApp).collect();
    // The connection that has this installation first.
    apps.sort_by_key(|c| c.installation.as_ref().map(|i| i.id) != Some(installation_id));
    for connection in apps {
        let Ok(jwt) = github_app::jwt(app_of(&connection)?, Utc::now()) else { continue };
        let provider = Provider::new(GitProvider::Github, &connection.base_url, &jwt);
        match provider.app_installation(installation_id).await {
            Ok(installation) => return Ok(Step::connected(adopt(store, connection, installation).await?)),
            // Not an installation of this app (or this app is gone).
            Err(ProviderError::NotFound(_) | ProviderError::Unauthorized(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(invalid(format!("no GitHub App of this server has the installation {installation_id}")))
}

/// What the provider sent the browser back with.
pub async fn callback(store: &Store, cb: &GitCallback) -> Result<Step, FlowError> {
    let state = cb.state.trim();
    if state.is_empty() {
        return match cb.installation_id {
            Some(id) => installation_changed(store, id).await,
            None => Err(invalid("state is required")),
        };
    }
    let Some(pending) = take(state) else {
        return Err(invalid("this authorization has expired or was already used: start again"));
    };
    let label = pending.provider().label();
    if let Some(error) = non_empty(&cb.error) {
        let why = non_empty(&cb.error_description).unwrap_or(error);
        return Err(FlowError::Provider(ProviderError::Unauthorized(format!(
            "{label} did not authorize Ferry: {why}"
        ))));
    }
    let code = || non_empty(&cb.code).ok_or_else(|| invalid(format!("{label} sent no code back")));
    match pending {
        Pending::GithubApp { base_url, webhook } => {
            let mut registration =
                Provider::anonymous(GitProvider::Github, &base_url).convert_manifest(code()?).await?;
            registration.webhook_secret = webhook_secret(&base_url, &registration, webhook).await;
            let connection = save_app(store, &base_url, registration).await?;
            install(store, connection).await
        }
        Pending::GithubInstall { connection_id } => {
            let connection = store.require_git_connection(&connection_id).await?;
            let app = app_of(&connection)?;
            let jwt = github_app::jwt(app, Utc::now())?;
            let provider = Provider::new(GitProvider::Github, &connection.base_url, &jwt);
            let installation = match cb.installation_id {
                Some(id) => provider.app_installation(id).await.map_err(|e| match e {
                    ProviderError::NotFound(_) => ProviderError::Unauthorized(format!(
                        "{id} is not an installation of the GitHub App '{}'",
                        app.slug
                    )),
                    other => other,
                })?,
                // An organization's owners still have to approve it.
                None if non_empty(&cb.setup_action) == Some("request") => {
                    return Err(invalid(format!(
                        "the installation of the GitHub App '{}' waits for an owner of the organization to approve \
                         it: finish connecting once it is approved",
                        app.slug
                    )));
                }
                None => pick(provider.app_installations().await?, &connection).ok_or_else(|| {
                    invalid(format!("the GitHub App '{}' is not installed on an account yet", app.slug))
                })?,
            };
            Ok(Step::connected(adopt(store, connection, installation).await?))
        }
        Pending::Oauth { connection_id, redirect_uri } => {
            let application = store.require_git_connection(&connection_id).await?;
            let (Some(client_id), Some(client_secret)) = (&application.client_id, &application.client_secret) else {
                return Err(FlowError::ApplicationRequired(format!(
                    "the {} has no OAuth application",
                    application.describe()
                )));
            };
            let tokens =
                oauth::exchange(&application.base_url, client_id, client_secret, code()?, &redirect_uri).await?;
            let (login, name) =
                Provider::new(application.provider, &application.base_url, &tokens.access_token).user().await?;
            if login.trim().is_empty() {
                return Err(FlowError::Provider(ProviderError::Unavailable(format!(
                    "{label} didn't say which account authorized the application"
                ))));
            }
            Ok(Step::connected(save_authorization(store, application, login, name, tokens).await?))
        }
    }
}

/// Connect the account a personal access token belongs to (replacing what
/// connected that account before). The flag says whether the connection is
/// new.
pub async fn connect_with_token(
    store: &Store,
    provider: GitProvider,
    base_url: &str,
    token: &str,
) -> Result<(GitConnection, bool), FlowError> {
    let account = Provider::new(provider, base_url, token).account().await?;
    if account.login.trim().is_empty() {
        return Err(FlowError::Provider(ProviderError::Unavailable(format!(
            "{} didn't say which account the token belongs to",
            provider.label()
        ))));
    }
    let _writing = WRITES.lock().await;
    match store.find_git_connection(provider, base_url, &account.login).await? {
        Some(mut existing) => {
            existing.auth = GitAuth::Token;
            existing.account = account.login;
            existing.account_name = account.name;
            existing.token = token.to_string();
            existing.refresh_token = None;
            existing.scopes = account.scopes;
            existing.token_expires_at = account.token_expires_at;
            existing.client_id = None;
            existing.client_secret = None;
            existing.app = None;
            existing.installation = None;
            access::forget(&existing.id);
            let saved = store.update_git_connection(&existing).await?;
            tracing::info!(connection = %saved.id, "replaced the token of the {}", saved.describe());
            Ok((saved, false))
        }
        None => {
            let mut connection = GitConnection::new(provider, base_url, account.login, token);
            connection.account_name = account.name;
            connection.scopes = account.scopes;
            connection.token_expires_at = account.token_expires_at;
            store.create_git_connection(&connection).await?;
            tracing::info!(connection = %connection.id, "connected the {}", connection.describe());
            Ok((connection, true))
        }
    }
}

/// Remove a connection and what is kept in memory for it. What it
/// authorized stays on the provider (a GitHub App, an OAuth grant, a token)
/// until it is removed there.
pub async fn disconnect(store: &Store, connection: &GitConnection) -> ferry_core::Result<()> {
    store.delete_git_connection(&connection.id).await?;
    access::forget(&connection.id);
    tracing::info!(connection = %connection.id, "disconnected the {}", connection.describe());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(provider: GitProvider) -> AuthorizeGit {
        AuthorizeGit {
            provider: Some(provider),
            redirect_uri: "http://localhost:7878/git/callback".into(),
            ..Default::default()
        }
    }

    fn state_of(url: &str) -> String {
        let (_, query) = url.split_once('?').expect("a query string");
        query.split('&').find_map(|p| p.strip_prefix("state=")).expect("a state").to_string()
    }

    #[test]
    fn instances() {
        assert_eq!(instance_url(GitProvider::Github, None).unwrap(), "https://github.com");
        assert_eq!(instance_url(GitProvider::Gitlab, Some("  ")).unwrap(), "https://gitlab.com");
        // A bare host means https.
        assert_eq!(
            instance_url(GitProvider::Gitlab, Some("GitLab.example.com/")).unwrap(),
            "https://gitlab.example.com"
        );
        assert_eq!(instance_url(GitProvider::Gitlab, Some("http://10.0.0.5:8929")).unwrap(), "http://10.0.0.5:8929");
        assert!(instance_url(GitProvider::Github, Some("ssh://git@ghe.example.com")).is_err());
        assert!(instance_url(GitProvider::Github, Some("user:pw@ghe.example.com")).is_err());
    }

    #[test]
    fn states_work_once_and_are_forgotten_when_there_are_too_many() {
        // Its own table: the tests of this module share the global one.
        let flows = Flows::default();
        let state = flows.remember(Pending::GithubApp { base_url: "https://github.com".into(), webhook: false });
        assert_eq!(state.len(), 48);
        assert!(matches!(flows.take(&state), Some(Pending::GithubApp { .. })));
        assert!(flows.take(&state).is_none());
        assert!(flows.take("never-issued").is_none());
        let first = flows.remember(Pending::GithubInstall { connection_id: "git-first".into() });
        std::thread::sleep(Duration::from_millis(5));
        let second = flows.remember(Pending::GithubInstall { connection_id: "git-second".into() });
        std::thread::sleep(Duration::from_millis(5));
        for _ in 0..MAX_FLOWS - 1 {
            flows.remember(Pending::GithubInstall { connection_id: "git-x".into() });
        }
        assert!(flows.take(&first).is_none(), "the oldest state was forgotten");
        assert!(flows.take(&second).is_some());
    }

    #[tokio::test]
    async fn github_starts_with_a_manifest_posted_to_github() {
        let store = Store::open_in_memory().await.unwrap();
        let Step::Form { url, fields } = start(&store, &request(GitProvider::Github), None).await.unwrap() else {
            panic!("expected a form");
        };
        assert!(url.starts_with("https://github.com/settings/apps/new?state="), "{url}");
        let manifest: serde_json::Value = serde_json::from_str(&fields["manifest"]).unwrap();
        assert_eq!(manifest["redirect_url"], "http://localhost:7878/git/callback");
        assert!(manifest.get("hook_attributes").is_none(), "{manifest}");
        assert!(matches!(
            take(&state_of(&url)),
            Some(Pending::GithubApp { base_url, webhook: false }) if base_url == "https://github.com"
        ));
        // A server GitHub can reach: the app is registered with its webhook.
        let hook = "https://ferry.example.com/hooks/github";
        let Step::Form { url, fields } = start(&store, &request(GitProvider::Github), Some(hook)).await.unwrap() else {
            panic!("expected a form");
        };
        let manifest: serde_json::Value = serde_json::from_str(&fields["manifest"]).unwrap();
        assert_eq!(manifest["hook_attributes"]["url"], hook);
        assert_eq!(manifest["default_events"], serde_json::json!(["push"]));
        assert!(matches!(take(&state_of(&url)), Some(Pending::GithubApp { webhook: true, .. })));
        // In an organization, on a GitHub Enterprise Server.
        let mut req = request(GitProvider::Github);
        req.organization = Some(" acme ".into());
        req.base_url = Some("ghe.example.com".into());
        let Step::Form { url, .. } = start(&store, &req, None).await.unwrap() else { panic!("expected a form") };
        assert!(url.starts_with("https://ghe.example.com/organizations/acme/settings/apps/new?state="), "{url}");
        req.organization = Some("acme/../x".into());
        assert!(matches!(start(&store, &req, None).await, Err(FlowError::Core(ferry_core::Error::Invalid(_)))));
        // Nothing is stored before GitHub answers.
        assert!(store.list_git_connections().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn gitlab_needs_an_application_then_remembers_it() {
        let store = Store::open_in_memory().await.unwrap();
        let err = start(&store, &request(GitProvider::Gitlab), None).await.unwrap_err();
        assert!(
            matches!(&err, FlowError::ApplicationRequired(m) if m.contains("read_api, read_repository")),
            "{err:?}"
        );
        let mut req = request(GitProvider::Gitlab);
        req.client_id = Some("app-id".into());
        assert!(matches!(start(&store, &req, None).await, Err(FlowError::Core(ferry_core::Error::Invalid(_)))));
        req.client_secret = Some("app-s".into());
        let Step::Redirect(url) = start(&store, &req, None).await.unwrap() else { panic!("expected a redirect") };
        assert!(url.starts_with("https://gitlab.com/oauth/authorize?client_id=app-id&redirect_uri="), "{url}");
        // The application waits as a pending connection...
        let pending = store.find_git_connection(GitProvider::Gitlab, "https://gitlab.com", "").await.unwrap().unwrap();
        assert_eq!((pending.auth, pending.is_connected()), (GitAuth::Oauth, false));
        // ...which later authorizations reuse, and a new application replaces.
        let Step::Redirect(again) = start(&store, &request(GitProvider::Gitlab), None).await.unwrap() else {
            panic!("expected a redirect")
        };
        assert!(again.contains("client_id=app-id&") && state_of(&again) != state_of(&url), "{again}");
        req.client_id = Some("other-id".into());
        let Step::Redirect(replaced) = start(&store, &req, None).await.unwrap() else { panic!("expected a redirect") };
        assert!(replaced.contains("client_id=other-id&"), "{replaced}");
        assert_eq!(store.list_git_connections().await.unwrap().len(), 1);
        // Resuming it by id works too.
        let mut resume = request(GitProvider::Gitlab);
        resume.provider = None;
        resume.connection_id = Some(pending.id.clone());
        assert!(
            matches!(start(&store, &resume, None).await, Ok(Step::Redirect(u)) if u.contains("client_id=other-id&"))
        );
    }

    #[tokio::test]
    async fn what_was_not_asked_for_is_refused() {
        let store = Store::open_in_memory().await.unwrap();
        let cb = |state: &str| GitCallback { state: state.into(), code: Some("c0de".into()), ..Default::default() };
        let err = callback(&store, &cb("made-up")).await.unwrap_err();
        assert!(
            matches!(&err, FlowError::Core(ferry_core::Error::Invalid(m)) if m.contains("expired or was already used")),
            "{err:?}"
        );
        assert!(matches!(callback(&store, &cb("  ")).await, Err(FlowError::Core(ferry_core::Error::Invalid(_)))));
        // The provider reports that the user refused: said as it is, and the state is spent.
        let Step::Form { url, .. } = start(&store, &request(GitProvider::Github), None).await.unwrap() else {
            panic!("expected a form")
        };
        let denied = GitCallback {
            state: state_of(&url),
            error: Some("access_denied".into()),
            error_description: Some("The user denied the request".into()),
            ..Default::default()
        };
        let err = callback(&store, &denied).await.unwrap_err();
        assert!(
            matches!(&err, FlowError::Provider(ProviderError::Unauthorized(m)) if m == "GitHub did not authorize Ferry: The user denied the request"),
            "{err:?}"
        );
        assert!(matches!(callback(&store, &denied).await, Err(FlowError::Core(_))));
        // Bad requests.
        let mut req = request(GitProvider::Github);
        req.redirect_uri = "javascript:alert(1)".into();
        assert!(matches!(start(&store, &req, None).await, Err(FlowError::Core(ferry_core::Error::Invalid(_)))));
        let mut req = request(GitProvider::Github);
        req.provider = None;
        assert!(matches!(start(&store, &req, None).await, Err(FlowError::Core(ferry_core::Error::Invalid(_)))));
        // A token connection has nothing to authorize in the browser.
        let pat = GitConnection::new(GitProvider::Github, "https://github.com", "octocat", "t");
        store.create_git_connection(&pat).await.unwrap();
        req.connection_id = Some(pat.id.clone());
        let err = start(&store, &req, None).await.unwrap_err();
        assert!(
            matches!(&err, FlowError::Core(ferry_core::Error::Invalid(m)) if m.contains("connected with an access token")),
            "{err:?}"
        );
    }

    #[test]
    fn the_installation_of_a_connection() {
        let installation = |id: u64, account: &str, suspended: bool| Installation {
            id,
            account: account.into(),
            url: None,
            repository_selection: None,
            suspended,
        };
        let app = GithubApp { id: 1, slug: "s".into(), url: "u".into(), private_key: "k".into(), webhook_secret: None };
        let mut c = GitConnection::github_app("https://github.com", "octocat", app, "id", "s");
        assert_eq!(pick(vec![], &c), None);
        // The only one, whoever it is on.
        assert_eq!(pick(vec![installation(1, "acme", false)], &c).map(|i| i.id), Some(1));
        // Among several: the account's own.
        let several = || vec![installation(1, "acme", false), installation(2, "OctoCat", false)];
        assert_eq!(pick(several(), &c).map(|i| i.id), Some(2));
        // The one it already has wins.
        c.installation = Some(GithubInstallation { id: 1, url: None, repository_selection: None });
        assert_eq!(pick(several(), &c).map(|i| i.id), Some(1));
        // Several strangers: none is picked.
        c.installation = None;
        assert_eq!(pick(vec![installation(1, "acme", false), installation(3, "initech", false)], &c), None);
    }
}
