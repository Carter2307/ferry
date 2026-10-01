//! `/api/v1/git` — git connections: accounts of GitHub / GitLab connected
//! with an access token, their repositories and branches.
//!
//! The token is stored to authenticate the clones of the services that use
//! the connection; no response ever contains it.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{ApiErrorBody, ConnectGit, GitBranch, GitConnectionView, GitRepositoryList};
use ferry_core::{Error, GitConnection, GitProvider, validate};
use http::StatusCode;
use serde::Deserialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, DeleteQuery};
use crate::locks;
use crate::providers::{Account, Provider, ProviderError};
use crate::views::git_connection_view;

/// Error code: the provider rejected a token (the one given, or a stored one).
pub const TOKEN_REJECTED: &str = "git_token_rejected";
/// Error code: the provider could not be reached, or answered an error.
pub const PROVIDER_UNAVAILABLE: &str = "git_provider_unavailable";

/// Key of the lock around "is this account already connected?" + write.
const CONNECT_LOCK: &str = "git-connections";

/// The error of a failed request made with a token the client just gave.
fn new_token_error(provider: GitProvider, base_url: &str, e: ProviderError) -> ApiError {
    match e {
        ProviderError::Unauthorized(m) => ApiError::new(
            StatusCode::BAD_REQUEST,
            TOKEN_REJECTED,
            format!("{m}: check that it was copied completely and has not expired"),
        ),
        ProviderError::Forbidden(m) => ApiError::new(StatusCode::BAD_REQUEST, TOKEN_REJECTED, m),
        ProviderError::NotFound(m) => ApiError::bad_request(format!(
            "{m}: {base_url} doesn't look like a {} instance (check base_url)",
            provider.label()
        )),
        ProviderError::RateLimited(m) | ProviderError::Unavailable(m) => unavailable(m),
    }
}

/// The error of a failed request made with a connection's stored token.
fn stored_token_error(connection: &GitConnection, e: ProviderError) -> ApiError {
    let again = format!("connect the {} again with a new token", connection.describe());
    match e {
        ProviderError::Unauthorized(m) => ApiError::new(
            StatusCode::CONFLICT,
            TOKEN_REJECTED,
            format!("{m}: the token has expired or was revoked — {again}"),
        ),
        ProviderError::Forbidden(m) => {
            ApiError::new(StatusCode::CONFLICT, TOKEN_REJECTED, format!("{m} — to give it more access, {again}"))
        }
        ProviderError::NotFound(m) => ApiError::not_found(m),
        ProviderError::RateLimited(m) | ProviderError::Unavailable(m) => unavailable(m),
    }
}

fn unavailable(message: String) -> ApiError {
    tracing::warn!("git provider request failed: {message}");
    ApiError::new(StatusCode::BAD_GATEWAY, PROVIDER_UNAVAILABLE, message)
}

/// `GET /api/v1/git/connections`
#[utoipa::path(
    get,
    path = "/api/v1/git/connections",
    tag = "git",
    operation_id = "listGitConnections",
    summary = "List git connections",
    description = "The connected GitHub / GitLab accounts, with the names of the services cloned through each. Tokens are never returned: `token_hint` is the end of the stored one.",
    responses((status = 200, description = "All git connections.", body = [GitConnectionView])),
)]
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<GitConnectionView>>> {
    let mut out = Vec::new();
    for connection in st.store.list_git_connections().await? {
        out.push(git_connection_view(&st.store, connection).await?);
    }
    Ok(Json(out))
}

/// The provider instance of a connect request: the given one (a missing
/// scheme means https), else the provider's public one.
fn instance_url(req: &ConnectGit) -> Result<String, Error> {
    match req.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) if u.contains("://") => validate::git_base_url(u),
        Some(u) => validate::git_base_url(&format!("https://{u}")),
        None => Ok(req.provider.default_base_url().to_string()),
    }
}

/// `POST /api/v1/git/connections`
#[utoipa::path(
    post,
    path = "/api/v1/git/connections",
    tag = "git",
    operation_id = "connectGit",
    summary = "Connect a git account",
    description = "Connects the account an access token belongs to: Ferry asks the provider who the token is, then stores it to list the account's repositories and to clone them (also the private ones) for the services that use the connection. On GitHub use a classic personal access token with the `repo` scope, or a fine-grained one with read access to Contents and Metadata; on GitLab one with the `read_api` and `read_repository` scopes. `base_url` selects a self-hosted instance (GitHub Enterprise Server, GitLab self-managed). Connecting an account that is already connected replaces its token (200 instead of 201). The token is never returned by the API.",
    request_body = ConnectGit,
    responses(
        (status = 201, description = "The new connection.", body = GitConnectionView),
        (status = 200, description = "The account was already connected: its token was replaced.", body = GitConnectionView),
        (status = 400, description = "Malformed token or `base_url`, or the provider rejected the token (code `git_token_rejected`).", body = ApiErrorBody),
        (status = 502, description = "The provider could not be reached, or answered an error (code `git_provider_unavailable`).", body = ApiErrorBody),
    ),
)]
pub async fn connect(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<ConnectGit>,
) -> ApiResult<(StatusCode, Json<GitConnectionView>)> {
    let token = validate::git_token(&req.token)?.to_string();
    let base_url = instance_url(&req)?;
    let provider = req.provider;
    // The provider's answer and the row: finish even if the client goes away.
    locks::detached(async move {
        let account = Provider::new(provider, &base_url, &token)
            .account()
            .await
            .map_err(|e| new_token_error(provider, &base_url, e))?;
        let Account { login, name, scopes, token_expires_at } = account;
        if login.trim().is_empty() {
            return Err(unavailable(format!("{} didn't say which account the token belongs to", provider.label())));
        }
        // One connection per account: checked and written under one lock.
        let _guard = locks::owner(CONNECT_LOCK).await;
        let (status, connection) = match st.store.find_git_connection(provider, &base_url, &login).await? {
            Some(mut existing) => {
                existing.account = login;
                existing.account_name = name;
                existing.token = token;
                existing.scopes = scopes;
                existing.token_expires_at = token_expires_at;
                let saved = st.store.update_git_connection(&existing).await?;
                tracing::info!(connection = %saved.id, "replaced the token of the {}", saved.describe());
                (StatusCode::OK, saved)
            }
            None => {
                let mut connection = GitConnection::new(provider, base_url, login, token);
                connection.account_name = name;
                connection.scopes = scopes;
                connection.token_expires_at = token_expires_at;
                st.store.create_git_connection(&connection).await?;
                tracing::info!(connection = %connection.id, "connected the {}", connection.describe());
                (StatusCode::CREATED, connection)
            }
        };
        Ok((status, Json(git_connection_view(&st.store, connection).await?)))
    })
    .await
}

/// `GET /api/v1/git/connections/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/git/connections/{id}",
    tag = "git",
    operation_id = "getGitConnection",
    summary = "Get a git connection",
    params(("id" = String, Path, description = "Git connection id.")),
    responses(
        (status = 200, description = "The connection.", body = GitConnectionView),
        (status = 404, description = "No such git connection.", body = ApiErrorBody),
    ),
)]
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<GitConnectionView>> {
    let connection = st.store.require_git_connection(id.trim()).await?;
    Ok(Json(git_connection_view(&st.store, connection).await?))
}

/// `DELETE /api/v1/git/connections/{id}?force=`
///
/// Refused (409) while services use the connection: their next deploys
/// would clone without credentials.
#[utoipa::path(
    delete,
    path = "/api/v1/git/connections/{id}",
    tag = "git",
    operation_id = "deleteGitConnection",
    summary = "Disconnect a git account",
    description = "Deletes the connection and its stored token (the token itself stays valid on the provider: revoke it there). Refused while services use it, unless `force=true`: those services keep their repository and clone without credentials from then on, which fails for private repositories.",
    params(
        ("id" = String, Path, description = "Git connection id."),
        ("force" = Option<bool>, Query, description = "Disconnect even though services use the connection."),
    ),
    responses(
        (status = 204, description = "Disconnected."),
        (status = 404, description = "No such git connection.", body = ApiErrorBody),
        (status = 409, description = "Services use it (the message lists them).", body = ApiErrorBody),
    ),
)]
pub async fn delete(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<DeleteQuery>,
) -> ApiResult<StatusCode> {
    let connection = st.store.require_git_connection(id.trim()).await?;
    let users: Vec<String> =
        st.store.git_connection_services(&connection.id).await?.into_iter().map(|s| s.name).collect();
    if !users.is_empty() {
        if !q.force {
            return Err(Error::conflict(format!(
                "the {} is used by {}: without it they clone without credentials, which fails for private repositories. Select another connection in their settings first, or disconnect with force=true",
                connection.describe(),
                users.join(", ")
            ))
            .into());
        }
        tracing::warn!(connection = %connection.id, users = %users.join(", "), "disconnecting a git account services use");
    }
    st.store.delete_git_connection(&connection.id).await?;
    tracing::info!(connection = %connection.id, "disconnected the {}", connection.describe());
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/git/connections/{id}/repositories`
#[utoipa::path(
    get,
    path = "/api/v1/git/connections/{id}/repositories",
    tag = "git",
    operation_id = "listGitRepositories",
    summary = "List the repositories of a git connection",
    description = "The repositories the connection's token can access, asked from the provider on every call: the account's own, its organizations' / groups' and those it collaborates on, most recently updated first. At most 1000 are listed (`truncated` says when there are more). Use a repository's `clone_url` as the `repo_url` of a service, with this connection as its `git_connection_id`.",
    params(("id" = String, Path, description = "Git connection id.")),
    responses(
        (status = 200, description = "The repositories.", body = GitRepositoryList),
        (status = 404, description = "No such git connection.", body = ApiErrorBody),
        (status = 409, description = "The provider rejected the stored token: expired, revoked or lacking access (code `git_token_rejected`). Connect the account again.", body = ApiErrorBody),
        (status = 502, description = "The provider could not be reached, or answered an error (code `git_provider_unavailable`).", body = ApiErrorBody),
    ),
)]
pub async fn repositories(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<GitRepositoryList>> {
    let connection = st.store.require_git_connection(id.trim()).await?;
    let (repositories, truncated) =
        Provider::of(&connection).repositories().await.map_err(|e| stored_token_error(&connection, e))?;
    Ok(Json(GitRepositoryList { repositories, truncated }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BranchesQuery {
    /// Full name of the repository, as listed (`owner/name`; GitLab subgroups included).
    pub repository: String,
}

/// `GET /api/v1/git/connections/{id}/branches?repository=`
#[utoipa::path(
    get,
    path = "/api/v1/git/connections/{id}/branches",
    tag = "git",
    operation_id = "listGitBranches",
    summary = "List the branches of a repository",
    description = "The branches of a repository the connection can access, asked from the provider on every call (at most 500).",
    params(("id" = String, Path, description = "Git connection id."), BranchesQuery),
    responses(
        (status = 200, description = "The branches.", body = [GitBranch]),
        (status = 400, description = "Malformed repository name.", body = ApiErrorBody),
        (status = 404, description = "No such git connection, or no such repository for its token.", body = ApiErrorBody),
        (status = 409, description = "The provider rejected the stored token (code `git_token_rejected`). Connect the account again.", body = ApiErrorBody),
        (status = 502, description = "The provider could not be reached, or answered an error (code `git_provider_unavailable`).", body = ApiErrorBody),
    ),
)]
pub async fn branches(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<BranchesQuery>,
) -> ApiResult<Json<Vec<GitBranch>>> {
    let connection = st.store.require_git_connection(id.trim()).await?;
    let repository = q.repository.trim();
    validate::git_repository_name(repository)?;
    if connection.provider == GitProvider::Github && repository.split('/').count() != 2 {
        return Err(Error::invalid(format!(
            "invalid repository '{repository}': GitHub repositories are named owner/name"
        ))
        .into());
    }
    let branches =
        Provider::of(&connection).branches(repository).await.map_err(|e| stored_token_error(&connection, e))?;
    Ok(Json(branches))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances() {
        let req = |provider, base_url: Option<&str>| ConnectGit {
            provider,
            token: "t".into(),
            base_url: base_url.map(str::to_string),
        };
        assert_eq!(instance_url(&req(GitProvider::Github, None)).unwrap(), "https://github.com");
        assert_eq!(instance_url(&req(GitProvider::Gitlab, Some("  "))).unwrap(), "https://gitlab.com");
        // A bare host means https.
        assert_eq!(
            instance_url(&req(GitProvider::Gitlab, Some("GitLab.example.com/"))).unwrap(),
            "https://gitlab.example.com"
        );
        assert_eq!(
            instance_url(&req(GitProvider::Gitlab, Some("http://10.0.0.5:8929"))).unwrap(),
            "http://10.0.0.5:8929"
        );
        assert!(instance_url(&req(GitProvider::Github, Some("ssh://git@ghe.example.com"))).is_err());
        assert!(instance_url(&req(GitProvider::Github, Some("user:pw@ghe.example.com"))).is_err());
    }

    #[test]
    fn provider_errors_never_answer_401() {
        // 401 means "the Ferry token is wrong" to API clients (the dashboard signs out).
        let connection = GitConnection::new(GitProvider::Github, "https://github.com", "octocat", "ghp_secret");
        let rejected = || ProviderError::Unauthorized("GitHub rejected the token: Bad credentials".into());
        let new = new_token_error(GitProvider::Github, "https://github.com", rejected());
        assert_eq!((new.status, new.body.error.code.as_str()), (StatusCode::BAD_REQUEST, TOKEN_REJECTED));
        let stored = stored_token_error(&connection, rejected());
        assert_eq!((stored.status, stored.body.error.code.as_str()), (StatusCode::CONFLICT, TOKEN_REJECTED));
        assert!(stored.body.error.message.contains("connect the GitHub account 'octocat' again"), "{stored}");
        assert!(!stored.body.error.message.contains("ghp_"), "{stored}");
        let down = stored_token_error(&connection, ProviderError::Unavailable("cannot reach GitHub".into()));
        assert_eq!((down.status, down.body.error.code.as_str()), (StatusCode::BAD_GATEWAY, PROVIDER_UNAVAILABLE));
        let limited = stored_token_error(&connection, ProviderError::RateLimited("slow down".into()));
        assert_eq!(limited.status, StatusCode::BAD_GATEWAY);
        let missing = stored_token_error(&connection, ProviderError::NotFound("repository 'a/b' not found".into()));
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        // A wrong instance address: the account endpoint isn't there.
        let wrong = new_token_error(
            GitProvider::Gitlab,
            "https://example.com",
            ProviderError::NotFound("GitLab answered 404 Not Found".into()),
        );
        assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
        assert!(wrong.body.error.message.contains("doesn't look like a GitLab instance"), "{wrong}");
    }
}
