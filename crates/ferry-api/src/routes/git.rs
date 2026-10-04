//! `/api/v1/git` — git connections: the GitHub / GitLab accounts this server
//! is authorized to read the repositories of, how they get authorized in the
//! browser, their repositories, and the branches of any repository.
//!
//! Connections belong to the server: a service names none, its repository
//! is cloned with the connection that serves its URL. No response ever
//! contains a secret (a token, an app's private key, a client secret).

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use ferry_build::{GitCredentials, RemoteError};
use ferry_core::dto::{
    ApiErrorBody, AuthorizeGit, ConnectGit, GitAuthorization, GitBranches, GitCallback, GitConnectionView,
    GitRepositoryList,
};
use ferry_core::{Config, Error, validate};
use ferry_scm::{FlowError, ProviderError, Step};
use http::StatusCode;
use serde::Deserialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, DeleteQuery};
use crate::locks;
use crate::routes::hooks;
use crate::views::{git_connection_users, git_connection_view, git_connection_view_of};

/// Error code: the provider refused an authorization — the one being made
/// (400), or the one a connection has stored (409).
pub const AUTHORIZATION_REJECTED: &str = "git_authorization_rejected";
/// Error code: the provider could not be reached, or answered an error.
pub const PROVIDER_UNAVAILABLE: &str = "git_provider_unavailable";
/// Error code: the provider needs an OAuth application created for this
/// server before an account can authorize it.
pub const APPLICATION_REQUIRED: &str = "git_application_required";
/// Error code: a repository's remote refused, wasn't found or didn't answer.
pub const REMOTE_UNREACHABLE: &str = "git_remote_unreachable";

/// How long a remote has to list its branches.
const BRANCHES_TIMEOUT: Duration = Duration::from_secs(20);

fn unavailable(message: String) -> ApiError {
    tracing::warn!("git provider request failed: {message}");
    ApiError::new(StatusCode::BAD_GATEWAY, PROVIDER_UNAVAILABLE, message)
}

fn internal(message: String) -> ApiError {
    Error::internal(message).into()
}

/// The error of a provider failure while an account is being authorized
/// (what the client just gave or did is refused). Never 401: that means
/// "the Ferry token is wrong" to API clients.
fn authorizing_error(e: ProviderError) -> ApiError {
    match e {
        ProviderError::Unauthorized(m) | ProviderError::Forbidden(m) => {
            ApiError::new(StatusCode::BAD_REQUEST, AUTHORIZATION_REJECTED, m)
        }
        ProviderError::NotFound(m) => {
            ApiError::bad_request(format!("{m}: is the address of the provider instance right?"))
        }
        ProviderError::RateLimited(m) | ProviderError::Unavailable(m) => unavailable(m),
        ProviderError::Internal(m) => internal(m),
    }
}

/// The error of a provider failure with what a connection has stored.
fn stored_error(e: ProviderError) -> ApiError {
    match e {
        ProviderError::Unauthorized(m) | ProviderError::Forbidden(m) => {
            ApiError::new(StatusCode::CONFLICT, AUTHORIZATION_REJECTED, m)
        }
        ProviderError::NotFound(m) => ApiError::not_found(m),
        ProviderError::RateLimited(m) | ProviderError::Unavailable(m) => unavailable(m),
        ProviderError::Internal(m) => internal(m),
    }
}

fn flow_error(e: FlowError) -> ApiError {
    match e {
        FlowError::Core(e) => e.into(),
        FlowError::ApplicationRequired(m) => ApiError::new(StatusCode::BAD_REQUEST, APPLICATION_REQUIRED, m),
        FlowError::Provider(e) => authorizing_error(e),
    }
}

/// Where GitHub can deliver the pushes of an account's repositories to this
/// server: the GitHub webhook endpoint on the address the dashboard is used
/// at (the origin of `redirect_uri`), else on the server's dashboard URL —
/// whichever is on the internet. `None` for a server GitHub can't reach.
fn github_webhook_url(config: &Config, redirect_uri: &str) -> Option<String> {
    let dashboard = config.dashboard_url();
    [Some(redirect_uri), dashboard.as_deref()]
        .into_iter()
        .flatten()
        .find_map(ferry_scm::github_app::public_origin)
        .map(|origin| format!("{origin}{}", hooks::GITHUB_PATH))
}

/// The API's answer for a step of a browser authorization.
async fn authorization(st: &AppState, step: Step) -> ApiResult<GitAuthorization> {
    Ok(match step {
        Step::Redirect(url) => GitAuthorization::redirect(url),
        Step::Form { url, fields } => GitAuthorization::form(url, fields),
        Step::Connected(connection) => {
            GitAuthorization::connected(git_connection_view_of(&st.store, *connection).await?)
        }
    })
}

/// `GET /api/v1/git/connections`
#[utoipa::path(
    get,
    path = "/api/v1/git/connections",
    tag = "git",
    operation_id = "listGitConnections",
    summary = "List git connections",
    description = "The GitHub / GitLab accounts this server is authorized to read the repositories of, with the names of the services whose repository each one clones. A `pending` connection was started in the browser and not finished. Secrets are never returned.",
    responses((status = 200, description = "All git connections.", body = [GitConnectionView])),
)]
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<GitConnectionView>>> {
    let connections = st.store.list_git_connections().await?;
    let mut users = git_connection_users(&st.store, &connections).await?;
    let views = connections
        .into_iter()
        .map(|c| {
            let services = users.remove(&c.id).unwrap_or_default();
            git_connection_view(c, services)
        })
        .collect();
    Ok(Json(views))
}

/// `POST /api/v1/git/authorize`
#[utoipa::path(
    post,
    path = "/api/v1/git/authorize",
    tag = "git",
    operation_id = "authorizeGit",
    summary = "Authorize a git account in the browser",
    description = "Starts connecting an account on the provider's own pages and answers with where to send the browser (`status: redirect`): a URL to navigate to (`method: get`) or to submit a form with `fields` to (`method: post`). When the provider is done it sends the browser back to `redirect_uri` (the dashboard's `/git/callback` page) with query parameters to hand to `POST /api/v1/git/callback`.\n\n**GitHub**: the browser posts a manifest to GitHub, which registers a private GitHub App for this server (on the user's account, or in `organization`) and then asks the account which repositories the app may read. No token is ever typed: the app's key mints short-lived tokens. When the dashboard is used at an address on the internet (the origin of `redirect_uri`, else the server's dashboard URL), the app also gets a webhook: GitHub delivers the pushes of those repositories to `/hooks/github`, and services with auto-deploy deploy on push with nothing to set up on a repository (`push_events` of the connection).\n\n**GitLab**: the account authorizes an OAuth application created for this server (scopes `read_api` and `read_repository`, redirect URI = `redirect_uri`). Give its `client_id` and `client_secret` the first time (400 `git_application_required` otherwise); they are kept for later authorizations.\n\nWith `connection_id`, resumes a `pending` connection or authorizes a connection again; the answer is `status: connected` when nothing is left to do (a GitHub App that is already installed).",
    request_body = AuthorizeGit,
    responses(
        (status = 200, description = "Where to send the browser, or the connection when it is already complete.", body = GitAuthorization),
        (status = 400, description = "Invalid `redirect_uri`, `base_url` or organization; a provider that needs its OAuth application first (code `git_application_required`); or the provider refused (code `git_authorization_rejected`).", body = ApiErrorBody),
        (status = 404, description = "No such git connection (`connection_id`).", body = ApiErrorBody),
        (status = 502, description = "The provider could not be reached, or answered an error (code `git_provider_unavailable`).", body = ApiErrorBody),
    ),
)]
pub async fn authorize(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<AuthorizeGit>,
) -> ApiResult<Json<GitAuthorization>> {
    // The provider's answer and the row: finish even if the client goes away.
    locks::detached(async move {
        let webhook_url = github_webhook_url(&st.config, &req.redirect_uri);
        let step = ferry_scm::start(&st.store, &req, webhook_url.as_deref()).await.map_err(flow_error)?;
        Ok(Json(authorization(&st, step).await?))
    })
    .await
}

/// `POST /api/v1/git/callback`
#[utoipa::path(
    post,
    path = "/api/v1/git/callback",
    tag = "git",
    operation_id = "gitCallback",
    summary = "Finish a step of a browser authorization",
    description = "Takes the query parameters the provider sent the browser back with (to the `redirect_uri` given to `POST /api/v1/git/authorize`): `state`, and `code` and / or `installation_id`. Answers with the next page to send the browser to (`status: redirect`: after GitHub registered the app, its installation page) or with the connection (`status: connected`). A `state` works once, for an hour. Without `state` but with an `installation_id` (GitHub sends the browser back after an installation was changed on its own pages), the connection of that installation is read again.",
    request_body = GitCallback,
    responses(
        (status = 200, description = "The next step, or the connection.", body = GitAuthorization),
        (status = 400, description = "Unknown, expired or already used `state`; a missing parameter; or the provider refused: the user denied the authorization, the code is no longer valid (code `git_authorization_rejected`).", body = ApiErrorBody),
        (status = 502, description = "The provider could not be reached, or answered an error (code `git_provider_unavailable`).", body = ApiErrorBody),
    ),
)]
pub async fn callback(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<GitCallback>,
) -> ApiResult<Json<GitAuthorization>> {
    locks::detached(async move {
        let step = ferry_scm::callback(&st.store, &req).await.map_err(flow_error)?;
        Ok(Json(authorization(&st, step).await?))
    })
    .await
}

/// `POST /api/v1/git/connections`
#[utoipa::path(
    post,
    path = "/api/v1/git/connections",
    tag = "git",
    operation_id = "connectGit",
    summary = "Connect a git account with an access token",
    description = "The alternative to authorizing in the browser (`POST /api/v1/git/authorize`), for scripts and for instances where no application can be registered: connects the account a personal access token belongs to. Ferry asks the provider who the token is, then stores it. On GitHub use a classic personal access token with the `repo` scope, or a fine-grained one with read access to Contents and Metadata; on GitLab one with the `read_api` and `read_repository` scopes. `base_url` selects a self-hosted instance (GitHub Enterprise Server, GitLab self-managed). An account that is already connected gets the token instead of what it had (200 instead of 201). The token is never returned by the API.",
    request_body = ConnectGit,
    responses(
        (status = 201, description = "The new connection.", body = GitConnectionView),
        (status = 200, description = "The account was already connected: it now uses this token.", body = GitConnectionView),
        (status = 400, description = "Malformed token or `base_url`, or the provider rejected the token (code `git_authorization_rejected`).", body = ApiErrorBody),
        (status = 502, description = "The provider could not be reached, or answered an error (code `git_provider_unavailable`).", body = ApiErrorBody),
    ),
)]
pub async fn connect(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<ConnectGit>,
) -> ApiResult<(StatusCode, Json<GitConnectionView>)> {
    let token = validate::git_token(&req.token)?.to_string();
    let base_url = ferry_scm::instance_url(req.provider, req.base_url.as_deref())?;
    let provider = req.provider;
    // The provider's answer and the row: finish even if the client goes away.
    locks::detached(async move {
        let (connection, created) =
            ferry_scm::connect_with_token(&st.store, provider, &base_url, &token).await.map_err(|e| match e {
                FlowError::Provider(ProviderError::Unauthorized(m)) => ApiError::new(
                    StatusCode::BAD_REQUEST,
                    AUTHORIZATION_REJECTED,
                    format!("{m}: check that it was copied completely and has not expired"),
                ),
                FlowError::Provider(ProviderError::NotFound(m)) => ApiError::bad_request(format!(
                    "{m}: {base_url} doesn't look like a {} instance (check base_url)",
                    provider.label()
                )),
                other => flow_error(other),
            })?;
        let status = if created { StatusCode::CREATED } else { StatusCode::OK };
        Ok((status, Json(git_connection_view_of(&st.store, connection).await?)))
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
    Ok(Json(git_connection_view_of(&st.store, connection).await?))
}

/// `DELETE /api/v1/git/connections/{id}?force=`
///
/// Refused (409) while it clones the repository of services: their next
/// deploys would clone without credentials.
#[utoipa::path(
    delete,
    path = "/api/v1/git/connections/{id}",
    tag = "git",
    operation_id = "deleteGitConnection",
    summary = "Disconnect a git account",
    description = "Deletes the connection and its secrets. What it authorized stays on the provider until it is removed there: the GitHub App (delete it in GitHub's settings), the OAuth grant or the token. Refused while services' repositories are cloned with it, unless `force=true`: those services keep their repository and are cloned without credentials from then on, which fails for private repositories.",
    params(
        ("id" = String, Path, description = "Git connection id."),
        ("force" = Option<bool>, Query, description = "Disconnect even though services are cloned with the connection."),
    ),
    responses(
        (status = 204, description = "Disconnected."),
        (status = 404, description = "No such git connection.", body = ApiErrorBody),
        (status = 409, description = "Services are cloned with it (the message lists them).", body = ApiErrorBody),
    ),
)]
pub async fn delete(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<DeleteQuery>,
) -> ApiResult<StatusCode> {
    let connection = st.store.require_git_connection(id.trim()).await?;
    let all = st.store.list_git_connections().await?;
    let users = git_connection_users(&st.store, &all).await?.remove(&connection.id).unwrap_or_default();
    if !users.is_empty() {
        if !q.force {
            return Err(Error::conflict(format!(
                "the {} clones the repository of {}: without it they are cloned without credentials, which fails for private repositories. Disconnect with force=true",
                connection.describe(),
                users.join(", ")
            ))
            .into());
        }
        tracing::warn!(connection = %connection.id, users = %users.join(", "), "disconnecting a git account services are cloned with");
    }
    ferry_scm::disconnect(&st.store, &connection).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/git/connections/{id}/repositories`
#[utoipa::path(
    get,
    path = "/api/v1/git/connections/{id}/repositories",
    tag = "git",
    operation_id = "listGitRepositories",
    summary = "List the repositories of a git connection",
    description = "The repositories the connection can read, asked from the provider on every call, most recently updated first: for a GitHub App the repositories the account allowed it to read (see `manage_url`); otherwise the account's own, its organizations' / groups' and those it collaborates on. At most 1000 are listed (`truncated` says when there are more). Use a repository's `clone_url` as the `repo_url` of a service: it is cloned with this connection.",
    params(("id" = String, Path, description = "Git connection id.")),
    responses(
        (status = 200, description = "The repositories.", body = GitRepositoryList),
        (status = 404, description = "No such git connection.", body = ApiErrorBody),
        (status = 409, description = "The connection is pending, or the provider no longer accepts its authorization: an expired or revoked token, an uninstalled app (code `git_authorization_rejected`). Connect the account again.", body = ApiErrorBody),
        (status = 502, description = "The provider could not be reached, or answered an error (code `git_provider_unavailable`).", body = ApiErrorBody),
    ),
)]
pub async fn repositories(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<GitRepositoryList>> {
    let connection = st.store.require_git_connection(id.trim()).await?;
    let (repositories, truncated) = ferry_scm::repositories(&st.store, &connection).await.map_err(stored_error)?;
    Ok(Json(GitRepositoryList { repositories, truncated }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BranchesQuery {
    /// The repository: any URL a service can deploy from (`https://…`,
    /// `git@host:owner/name.git`, a path on the server).
    pub repo_url: String,
}

/// `GET /api/v1/git/branches?repo_url=`
#[utoipa::path(
    get,
    path = "/api/v1/git/branches",
    tag = "git",
    operation_id = "listGitBranches",
    summary = "List the branches of a repository",
    description = "The branches of a repository and its default one, asked from the repository's remote itself (`git ls-remote`) on every call. Works for any repository URL a service can deploy from; it is read the way a deploy would clone it: with the git connection that serves the URL when there is one (`connection_id` in the answer), with the credentials the URL carries, or without any.",
    params(BranchesQuery),
    responses(
        (status = 200, description = "The branches.", body = GitBranches),
        (status = 400, description = "Malformed repository URL.", body = ApiErrorBody),
        (status = 502, description = "The remote refused, wasn't found or didn't answer in time (code `git_remote_unreachable`): the message says why, and what to do when it looks like a private repository.", body = ApiErrorBody),
    ),
)]
pub async fn branches(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<BranchesQuery>,
) -> ApiResult<Json<GitBranches>> {
    let repo_url = q.repo_url.trim();
    validate::repo_url(repo_url)?;
    let access = ferry_scm::repo_access(&st.store, repo_url).await?;
    let connection = access.as_ref().map(|a| &a.connection);
    // A connection that can't produce a token: the remote is still asked
    // (a public repository answers); its error is told with the failure.
    let (credentials, token_error) = match &access {
        Some(a) => match &a.token {
            Ok(token) => (Some(GitCredentials { username: a.username().to_string(), password: token.clone() }), None),
            Err(e) => (None, Some(format!("the {} can't be used: {e}", a.connection.describe()))),
        },
        None => (None, None),
    };
    let remote =
        ferry_build::remote_branches(repo_url, credentials.as_ref(), BRANCHES_TIMEOUT).await.map_err(|e| match e {
            RemoteError::Invalid(m) => ApiError::bad_request(m),
            RemoteError::Unreachable(m) => {
                let advice = token_error.clone().or_else(|| ferry_scm::access_hint(&m, connection, repo_url));
                let message = match advice {
                    Some(advice) => format!("{m} — {advice}"),
                    None => m,
                };
                ApiError::new(StatusCode::BAD_GATEWAY, REMOTE_UNREACHABLE, message)
            }
        })?;
    let mut branches = remote.branches;
    branches.sort();
    // The default branch first.
    if let Some(at) = remote.default.as_ref().and_then(|d| branches.iter().position(|b| b == d)) {
        let default = branches.remove(at);
        branches.insert(0, default);
    }
    let connection_id = if credentials.is_some() { access.map(|a| a.connection.id) } else { None };
    Ok(Json(GitBranches { default_branch: remote.default, branches, connection_id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_errors_never_answer_401() {
        // 401 means "the Ferry token is wrong" to API clients (the dashboard signs out).
        let rejected = || ProviderError::Unauthorized("GitHub rejected the token: Bad credentials".into());
        let new = authorizing_error(rejected());
        assert_eq!((new.status, new.body.error.code.as_str()), (StatusCode::BAD_REQUEST, AUTHORIZATION_REJECTED));
        let stored = stored_error(rejected());
        assert_eq!((stored.status, stored.body.error.code.as_str()), (StatusCode::CONFLICT, AUTHORIZATION_REJECTED));
        let forbidden = stored_error(ProviderError::Forbidden("GitHub refused the request".into()));
        assert_eq!(forbidden.status, StatusCode::CONFLICT);
        let down = stored_error(ProviderError::Unavailable("cannot reach GitHub".into()));
        assert_eq!((down.status, down.body.error.code.as_str()), (StatusCode::BAD_GATEWAY, PROVIDER_UNAVAILABLE));
        let limited = authorizing_error(ProviderError::RateLimited("slow down".into()));
        assert_eq!(limited.status, StatusCode::BAD_GATEWAY);
        let missing = stored_error(ProviderError::NotFound("repository 'a/b' not found".into()));
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        // A wrong instance address: the provider's endpoints aren't there.
        let wrong = authorizing_error(ProviderError::NotFound("GitLab answered 404 Not Found".into()));
        assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
        assert!(wrong.body.error.message.contains("is the address of the provider instance right?"), "{wrong}");
        // Ferry's own failures are 500s.
        let broken = stored_error(ProviderError::Internal("the GitHub App's private key can't be read".into()));
        assert_eq!(broken.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn github_delivers_pushes_to_an_address_on_the_internet() {
        let hook = |config: &Config, redirect: &str| github_webhook_url(config, redirect);
        // A server on a laptop: nothing GitHub could reach.
        let local = Config::default();
        assert_eq!(hook(&local, "http://localhost:7878/git/callback"), None);
        assert_eq!(hook(&local, "http://192.168.1.20:7878/git/callback"), None);
        // The address the dashboard is used at.
        assert_eq!(
            hook(&local, "https://ferry.example.com/git/callback").as_deref(),
            Some("https://ferry.example.com/hooks/github")
        );
        // The dashboard reached through a tunnel: the server's own address.
        let public = Config {
            dashboard_host: Some("ferry.apps.example.com".into()),
            acme_email: Some("ops@example.com".into()),
            proxy_https_addr: Some("0.0.0.0:443".parse().unwrap()),
            ..Config::default()
        };
        assert_eq!(
            hook(&public, "http://localhost:7878/git/callback").as_deref(),
            Some("https://ferry.apps.example.com/hooks/github")
        );
        assert_eq!(
            hook(&public, "https://ferry.example.org:8443/git/callback").as_deref(),
            Some("https://ferry.example.org:8443/hooks/github")
        );
    }

    #[test]
    fn flow_errors() {
        let needed = flow_error(FlowError::ApplicationRequired("GitLab needs an OAuth application".into()));
        assert_eq!((needed.status, needed.body.error.code.as_str()), (StatusCode::BAD_REQUEST, APPLICATION_REQUIRED));
        let invalid = flow_error(FlowError::Core(Error::invalid("state is required")));
        assert_eq!((invalid.status, invalid.body.error.code.as_str()), (StatusCode::BAD_REQUEST, "invalid_request"));
        let gone = flow_error(FlowError::Core(Error::not_found("git connection", "git-x")));
        assert_eq!(gone.status, StatusCode::NOT_FOUND);
        let denied =
            flow_error(FlowError::Provider(ProviderError::Unauthorized("GitLab did not authorize Ferry".into())));
        assert_eq!((denied.status, denied.body.error.code.as_str()), (StatusCode::BAD_REQUEST, AUTHORIZATION_REJECTED));
    }
}
