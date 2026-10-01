//! Tokens to read repositories with, whatever the connection's kind: a
//! personal access token as it is, an OAuth access token renewed shortly
//! before it expires, and installation tokens of a GitHub App minted when
//! needed (and kept in memory for their hour).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, Duration, Utc};
use ferry_core::dto::GitRepository;
use ferry_core::{GitAuth, GitConnection, GitProvider, Store, git_connection_for};

use crate::provider::{InstallationToken, Provider, ProviderError};
use crate::{github_app, oauth};

/// A token is renewed this long before it expires: what it is handed to (a
/// clone, a listing) should not outlive it.
const RENEW_BEFORE: Duration = Duration::minutes(5);

/// Installation tokens of GitHub Apps, by connection id.
static INSTALLATION_TOKENS: LazyLock<Mutex<HashMap<String, (u64, InstallationToken)>>> = LazyLock::new(Mutex::default);

/// One renewal at a time per connection: a refresh token only works once.
static RENEWALS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Mutex::default);

fn renewal_lock(connection_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = RENEWALS.lock().unwrap_or_else(|e| e.into_inner());
    locks.entry(connection_id.to_string()).or_default().clone()
}

fn expiring(expires_at: Option<DateTime<Utc>>) -> bool {
    expires_at.is_some_and(|at| at - Utc::now() < RENEW_BEFORE)
}

fn cached_installation_token(connection_id: &str, installation_id: u64) -> Option<String> {
    let cache = INSTALLATION_TOKENS.lock().unwrap_or_else(|e| e.into_inner());
    let (id, token) = cache.get(connection_id)?;
    (*id == installation_id && !expiring(Some(token.expires_at))).then(|| token.token.clone())
}

/// Forget what is kept in memory for a connection (it was removed, or
/// authorized again).
pub fn forget(connection_id: &str) {
    INSTALLATION_TOKENS.lock().unwrap_or_else(|e| e.into_inner()).remove(connection_id);
    RENEWALS.lock().unwrap_or_else(|e| e.into_inner()).remove(connection_id);
}

fn not_authorized(connection: &GitConnection) -> ProviderError {
    ProviderError::Unauthorized(format!("the {} is not authorized yet: finish connecting it", connection.describe()))
}

/// A token that reads repositories through `connection` right now.
///
/// [`ProviderError::Unauthorized`] when the authorization is gone (the
/// refresh token was revoked, the app uninstalled): the account has to be
/// connected again.
pub async fn token(store: &Store, connection: &GitConnection) -> Result<String, ProviderError> {
    if !connection.is_connected() {
        return Err(not_authorized(connection));
    }
    match connection.auth {
        GitAuth::Token => Ok(connection.token.clone()),
        GitAuth::Oauth => oauth_token(store, connection).await,
        GitAuth::GithubApp => installation_token(connection).await,
    }
}

async fn oauth_token(store: &Store, connection: &GitConnection) -> Result<String, ProviderError> {
    if !expiring(connection.token_expires_at) {
        return Ok(connection.token.clone());
    }
    let lock = renewal_lock(&connection.id);
    let _renewing = lock.lock().await;
    // Somebody else may have renewed it while we waited.
    let current = store.require_git_connection(&connection.id).await?;
    if !expiring(current.token_expires_at) {
        return Ok(current.token);
    }
    let (Some(client_id), Some(client_secret), Some(refresh_token)) =
        (&current.client_id, &current.client_secret, &current.refresh_token)
    else {
        // Nothing to renew it with: the provider says whether it still works.
        return Ok(current.token);
    };
    match oauth::refresh(&current.base_url, client_id, client_secret, refresh_token).await {
        Ok(tokens) => {
            // Servers that don't rotate refresh tokens keep the one we have.
            let next = tokens.refresh_token.as_deref().unwrap_or(refresh_token);
            store.set_git_tokens(&current.id, &tokens.access_token, Some(next), tokens.expires_at).await?;
            tracing::debug!(connection = %current.id, "renewed the access token of the {}", current.describe());
            Ok(tokens.access_token)
        }
        Err(ProviderError::Unauthorized(m)) => {
            Err(ProviderError::Unauthorized(format!("{m} — authorize the {} again", current.describe())))
        }
        Err(e) => Err(e),
    }
}

async fn installation_token(connection: &GitConnection) -> Result<String, ProviderError> {
    let (Some(app), Some(installation)) = (&connection.app, &connection.installation) else {
        return Err(not_authorized(connection));
    };
    if let Some(token) = cached_installation_token(&connection.id, installation.id) {
        return Ok(token);
    }
    let lock = renewal_lock(&connection.id);
    let _minting = lock.lock().await;
    if let Some(token) = cached_installation_token(&connection.id, installation.id) {
        return Ok(token);
    }
    let jwt = github_app::jwt(app, Utc::now())?;
    let minted =
        Provider::new(GitProvider::Github, &connection.base_url, &jwt).installation_token(installation.id).await;
    match minted {
        Ok(token) => {
            let value = token.token.clone();
            let mut cache = INSTALLATION_TOKENS.lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(connection.id.clone(), (installation.id, token));
            Ok(value)
        }
        Err(ProviderError::NotFound(_)) => Err(ProviderError::Unauthorized(format!(
            "the GitHub App '{}' is no longer installed on '{}': connect the account again",
            app.slug, connection.account
        ))),
        Err(ProviderError::Unauthorized(m)) => Err(ProviderError::Unauthorized(format!(
            "{m} — the GitHub App '{}' may have been deleted: connect the account again",
            app.slug
        ))),
        Err(e) => Err(e),
    }
}

/// The repositories `connection` can read, most recently updated first,
/// and whether the listing was cut short.
pub async fn repositories(
    store: &Store,
    connection: &GitConnection,
) -> Result<(Vec<GitRepository>, bool), ProviderError> {
    let token = token(store, connection).await?;
    let provider = Provider::new(connection.provider, &connection.base_url, &token);
    let listed = match connection.auth {
        GitAuth::GithubApp => provider.installation_repositories().await,
        GitAuth::Oauth | GitAuth::Token => provider.repositories().await,
    };
    // Say what to do about a token the provider no longer takes.
    let account = connection.describe();
    listed.map_err(|e| match (e, connection.auth) {
        (ProviderError::Unauthorized(m), GitAuth::Token) => ProviderError::Unauthorized(format!(
            "{m}: the token has expired or was revoked — connect the {account} again with a new token"
        )),
        (ProviderError::Unauthorized(m), GitAuth::Oauth) => {
            ProviderError::Unauthorized(format!("{m} — authorize the {account} again"))
        }
        (ProviderError::Unauthorized(m), GitAuth::GithubApp) => {
            // Revoked before its hour: mint another one next time.
            forget(&connection.id);
            ProviderError::Unauthorized(format!("{m} — connect the {account} again"))
        }
        (ProviderError::Forbidden(m), GitAuth::Token) => ProviderError::Forbidden(format!(
            "{m} — to give it more access, connect the {account} again with a new token"
        )),
        (ProviderError::Forbidden(m), GitAuth::Oauth) => {
            ProviderError::Forbidden(format!("{m} — to give it more access, authorize the {account} again"))
        }
        (other, _) => other,
    })
}

/// What to do about a clone or a listing the remote refused, in one
/// sentence — `message` is git's (lowercase or not). `None` when the
/// failure doesn't look like an access problem, or when there is nothing a
/// connection could change (ssh, local paths, credentials in the URL).
pub fn access_hint(message: &str, connection: Option<&GitConnection>, repo_url: &str) -> Option<String> {
    let m = message.to_ascii_lowercase();
    // Wrong credentials, none where some are needed (git can't prompt: which
    // of the two messages depends on its version), or a private repository
    // the remote won't admit to.
    let refused = m.contains("authentication failed")
        || m.contains("unable to get password")
        || m.contains("could not read username")
        || m.contains("could not read password")
        || m.contains("access denied")
        || m.contains("returned error: 403")
        || (m.contains("repository") && m.contains("not found"));
    if !refused {
        return None;
    }
    match connection {
        Some(c) if c.auth == GitAuth::GithubApp => Some(format!(
            "the GitHub App of the {} may not be allowed to read this repository: add the repository to the app's \
             installation on GitHub",
            c.describe()
        )),
        Some(c) if c.auth == GitAuth::Oauth => Some(format!(
            "the {} may not have access to this repository, or its authorization was revoked: check its access, or \
             connect the account again",
            c.describe()
        )),
        Some(c) => Some(format!(
            "the token of the {} may have expired, been revoked or lack access to this repository: connect the \
             account again with a new token",
            c.describe()
        )),
        // Credentials in the URL are the user's own business.
        None if ferry_core::git::parse_http_url(repo_url).is_some() => Some(
            "if the repository is private, connect its GitHub or GitLab account to this server: it is then cloned \
             with that account"
                .to_string(),
        ),
        None => None,
    }
}

/// How a repository is read: the connection that serves it, and one of its
/// tokens (or why none could be had).
#[derive(Debug)]
pub struct RepoAccess {
    pub connection: GitConnection,
    pub token: Result<String, ProviderError>,
}

impl RepoAccess {
    /// The username git sends with the token.
    pub fn username(&self) -> &'static str {
        self.connection.provider.git_username()
    }
}

/// What reads `repo_url`: the connection that serves it
/// ([`git_connection_for`]), if any. `None`: the repository is read without
/// credentials (or with the ones its URL carries).
pub async fn repo_access(store: &Store, repo_url: &str) -> ferry_core::Result<Option<RepoAccess>> {
    let connections = store.list_git_connections().await?;
    let Some(connection) = git_connection_for(&connections, repo_url).cloned() else { return Ok(None) };
    let token = token(store, &connection).await;
    Ok(Some(RepoAccess { connection, token }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn personal_tokens_are_used_as_they_are_and_pending_connections_refused() {
        let store = Store::open_in_memory().await.unwrap();
        let pat = GitConnection::new(GitProvider::Github, "https://github.com", "octocat", "a-token");
        store.create_git_connection(&pat).await.unwrap();
        assert_eq!(token(&store, &pat).await.unwrap(), "a-token");
        let pending = GitConnection::oauth_application(GitProvider::Gitlab, "https://gitlab.com", "id", "s");
        let err = token(&store, &pending).await.unwrap_err();
        assert_eq!(
            err,
            ProviderError::Unauthorized("the GitLab application is not authorized yet: finish connecting it".into())
        );

        // What reads a repository: the connection of its host, or nothing.
        let access = repo_access(&store, "https://github.com/octocat/app.git").await.unwrap().unwrap();
        assert_eq!((access.connection.id.as_str(), access.username()), (pat.id.as_str(), "x-access-token"));
        assert_eq!(access.token.as_deref(), Ok("a-token"));
        assert!(repo_access(&store, "https://gitlab.com/octocat/app.git").await.unwrap().is_none());
        assert!(repo_access(&store, "git@github.com:octocat/app.git").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oauth_tokens_that_are_still_good_are_not_renewed() {
        let store = Store::open_in_memory().await.unwrap();
        // The instance can't be reached: a renewal would fail.
        let mut c = GitConnection::oauth_application(GitProvider::Gitlab, "http://127.0.0.1:9", "id", "s");
        c.account = "tanuki".into();
        c.token = "access".into();
        c.refresh_token = Some("refresh".into());
        c.token_expires_at = Some(Utc::now() + Duration::hours(1));
        store.create_git_connection(&c).await.unwrap();
        assert_eq!(token(&store, &c).await.unwrap(), "access");
        // No expiry known: used as it is.
        c.token_expires_at = None;
        assert_eq!(token(&store, &c).await.unwrap(), "access");
        // About to expire: renewed, which fails here with the reason.
        c.token_expires_at = Some(Utc::now() + Duration::minutes(1));
        let c = store.update_git_connection(&c).await.unwrap();
        let err = token(&store, &c).await.unwrap_err();
        assert!(
            matches!(&err, ProviderError::Unavailable(m) if m.starts_with("cannot reach GitLab at http://127.0.0.1:9")),
            "{err}"
        );
        // Without a refresh token there is nothing to renew with.
        let mut c = c;
        c.refresh_token = None;
        let c = store.update_git_connection(&c).await.unwrap();
        assert_eq!(token(&store, &c).await.unwrap(), "access");
    }
}
