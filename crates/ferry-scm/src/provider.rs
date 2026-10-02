//! Clients of the git providers' HTTP APIs (GitHub, GitLab): who a token
//! belongs to and which repositories it can read, the registration and
//! installations of a GitHub App, and the OAuth token endpoint.
//!
//! A token only ever travels in the `Authorization` header of requests to
//! its own provider instance; no secret appears in an error or a log line.

use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use ferry_core::GitProvider;
use ferry_core::dto::GitRepository;
use futures::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode, Url, header};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Repositories are listed 100 per page, up to this many pages.
pub const MAX_REPOSITORY_PAGES: u32 = 10;
/// Installations of a GitHub App are listed 100 per page, up to this many pages.
const MAX_INSTALLATION_PAGES: u32 = 3;
const PER_PAGE: &str = "100";
/// Pages requested at once, after the first one said how many there are.
const CONCURRENT_PAGES: usize = 4;

static HTTP: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("ferry/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
});

/// Why a provider request failed. Messages name the provider, never a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The provider rejected the credentials (mistyped, expired, revoked).
    Unauthorized(String),
    /// The credentials are valid but may not do this (a missing scope or
    /// permission).
    Forbidden(String),
    /// No such thing on the provider (or the credentials can't see it).
    NotFound(String),
    /// Too many requests with these credentials.
    RateLimited(String),
    /// The provider can't be reached, or answered something unexpected.
    Unavailable(String),
    /// Ferry's own failure (its database, a key it can't read).
    Internal(String),
}

impl ProviderError {
    pub fn message(&self) -> &str {
        match self {
            ProviderError::Unauthorized(m)
            | ProviderError::Forbidden(m)
            | ProviderError::NotFound(m)
            | ProviderError::RateLimited(m)
            | ProviderError::Unavailable(m)
            | ProviderError::Internal(m) => m,
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl From<ferry_core::Error> for ProviderError {
    fn from(e: ferry_core::Error) -> Self {
        ProviderError::Internal(e.to_string())
    }
}

/// The account a token belongs to, and what the provider says about the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub login: String,
    pub name: Option<String>,
    pub scopes: Vec<String>,
    pub token_expires_at: Option<DateTime<Utc>>,
}

/// A GitHub App as registered from a manifest, with its secrets.
#[derive(Clone)]
pub struct AppRegistration {
    pub id: u64,
    pub slug: String,
    /// The app's public page.
    pub url: String,
    pub client_id: String,
    pub client_secret: String,
    /// PEM private key.
    pub private_key: String,
    pub webhook_secret: Option<String>,
    /// Login of the user or organization the app belongs to.
    pub owner: String,
}

/// An installation of a GitHub App on an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub id: u64,
    /// Login of the account it is installed on.
    pub account: String,
    /// Its settings page.
    pub url: Option<String>,
    /// `all` or `selected`.
    pub repository_selection: Option<String>,
    pub suspended: bool,
}

/// A token of a GitHub App installation (valid for an hour).
#[derive(Clone)]
pub struct InstallationToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// What an OAuth token endpoint answers.
#[derive(Clone)]
pub struct OAuthTokens {
    pub access_token: String,
    /// Renews the access token (once: the answer carries its replacement).
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

// Secrets stay out of `{:?}`.
impl std::fmt::Debug for AppRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppRegistration")
            .field("id", &self.id)
            .field("slug", &self.slug)
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for InstallationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallationToken").field("expires_at", &self.expires_at).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

/// One instance of a provider, seen through one bearer token: a personal
/// or OAuth access token, the JWT of a GitHub App or the token of one of its
/// installations. Without a token, for the endpoints that take none.
pub struct Provider<'a> {
    kind: GitProvider,
    /// Web URL of the instance (normalized: no trailing slash).
    base_url: &'a str,
    token: &'a str,
}

/// One page of a listing.
struct Page<P> {
    body: P,
    /// Number of the last page, when the provider says.
    last: Option<u32>,
    /// There is a page after this one.
    more: bool,
}

impl<'a> Provider<'a> {
    pub fn new(kind: GitProvider, base_url: &'a str, token: &'a str) -> Self {
        Provider { kind, base_url, token }
    }

    /// For the endpoints that authenticate with something else than a
    /// bearer token (a manifest code, an OAuth client secret).
    pub fn anonymous(kind: GitProvider, base_url: &'a str) -> Self {
        Provider { kind, base_url, token: "" }
    }

    fn label(&self) -> &'static str {
        self.kind.label()
    }

    /// Root of the instance's REST API.
    fn api_url(&self) -> String {
        match self.kind {
            GitProvider::Github if self.base_url == GitProvider::Github.default_base_url() => {
                "https://api.github.com".to_string()
            }
            // GitHub Enterprise Server.
            GitProvider::Github => format!("{}/api/v3", self.base_url),
            GitProvider::Gitlab => format!("{}/api/v4", self.base_url),
        }
    }

    fn url(&self, root: &str, segments: &[&str], query: &[(&str, &str)]) -> Result<Url, ProviderError> {
        let invalid = || ProviderError::Unavailable(format!("invalid {} address {}", self.label(), self.base_url));
        let mut url = Url::parse(root).map_err(|_| invalid())?;
        url.path_segments_mut().map_err(|_| invalid())?.pop_if_empty().extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    /// `<api>/<segments...>?<query>`; a segment may contain `/` (it is escaped).
    fn endpoint(&self, segments: &[&str], query: &[(&str, &str)]) -> Result<Url, ProviderError> {
        self.url(&self.api_url(), segments, query)
    }

    fn client(&self) -> Result<&'static reqwest::Client, ProviderError> {
        HTTP.as_ref()
            .map_err(|e| ProviderError::Unavailable(format!("cannot set up the HTTP client for {}: {e}", self.label())))
    }

    async fn send(&self, request: RequestBuilder, url: &Url) -> Result<Response, ProviderError> {
        let request = if self.token.is_empty() { request } else { request.bearer_auth(self.token) };
        let request = match self.kind {
            GitProvider::Github => request
                .header(header::ACCEPT, "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28"),
            GitProvider::Gitlab => request.header(header::ACCEPT, "application/json"),
        };
        match request.send().await {
            Ok(response) if response.status().is_success() => Ok(response),
            Ok(response) => Err(self.failure(response).await),
            // Scheme and port included: a bare host was read as https.
            Err(e) => Err(ProviderError::Unavailable(format!(
                "cannot reach {} at {}: {}",
                self.label(),
                url.origin().ascii_serialization(),
                root_cause(&e)
            ))),
        }
    }

    async fn get(&self, url: Url) -> Result<Response, ProviderError> {
        self.send(self.client()?.get(url.clone()), &url).await
    }

    async fn post(&self, url: Url) -> Result<Response, ProviderError> {
        self.send(self.client()?.post(url.clone()), &url).await
    }

    /// The provider's own words for a failure, with this token masked
    /// (providers don't echo tokens; mask it anyway).
    fn detail(&self, body: &str) -> String {
        match api_message(body) {
            Some(m) if self.token.is_empty() => format!(": {m}"),
            Some(m) => format!(": {}", m.replace(self.token, "***")),
            None => String::new(),
        }
    }

    /// The error of a non-2xx answer.
    async fn failure(&self, response: Response) -> ProviderError {
        let status = response.status();
        let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && (header_str(&response, "x-ratelimit-remaining") == Some("0")
                    || response.headers().contains_key(header::RETRY_AFTER)));
        let detail = self.detail(&response.text().await.unwrap_or_default());
        let label = self.label();
        match status {
            _ if rate_limited => {
                ProviderError::RateLimited(format!("{label} is rate limiting this token: try again in a few minutes"))
            }
            StatusCode::UNAUTHORIZED => ProviderError::Unauthorized(format!("{label} rejected the token{detail}")),
            StatusCode::FORBIDDEN => ProviderError::Forbidden(format!("{label} refused the request{detail}")),
            StatusCode::NOT_FOUND => ProviderError::NotFound(format!("{label} answered 404 Not Found{detail}")),
            other => ProviderError::Unavailable(format!("{label} answered {other}{detail}")),
        }
    }

    async fn json<T: DeserializeOwned>(&self, response: Response) -> Result<T, ProviderError> {
        response.json::<T>().await.map_err(|e| {
            ProviderError::Unavailable(format!(
                "{} sent an answer Ferry doesn't understand: {}",
                self.label(),
                root_cause(&e)
            ))
        })
    }

    async fn page<P: DeserializeOwned>(&self, url: Url) -> Result<Page<P>, ProviderError> {
        let response = self.get(url).await?;
        let (last, more) = match self.kind {
            GitProvider::Github => {
                let link = header_str(&response, "link").unwrap_or_default();
                (link_page(link, "last"), link_page(link, "next").is_some())
            }
            GitProvider::Gitlab => (
                header_str(&response, "x-total-pages").and_then(|p| p.trim().parse().ok()),
                header_str(&response, "x-next-page").is_some_and(|p| !p.trim().is_empty()),
            ),
        };
        Ok(Page { body: self.json(response).await?, last, more })
    }

    /// Every item of a paginated listing, up to `max_pages` pages, in the
    /// provider's order: `items` takes them out of a page's body. The flag
    /// says whether pages were left out.
    async fn all_pages<P, T, F, I>(&self, max_pages: u32, url_of: F, items: I) -> Result<(Vec<T>, bool), ProviderError>
    where
        P: DeserializeOwned,
        F: Fn(u32) -> Result<Url, ProviderError>,
        I: Fn(P) -> Vec<T>,
    {
        let first = self.page::<P>(url_of(1)?).await?;
        let mut out = items(first.body);
        if !first.more {
            return Ok((out, false));
        }
        let url_of = &url_of;
        match first.last {
            // The number of pages is known: fetch the rest a few at a time.
            Some(last) => {
                let rest = futures::stream::iter(2..=last.min(max_pages))
                    .map(|n| async move { self.page::<P>(url_of(n)?).await })
                    .buffered(CONCURRENT_PAGES);
                let mut rest = std::pin::pin!(rest);
                while let Some(page) = rest.next().await {
                    out.extend(items(page?.body));
                }
                Ok((out, last > max_pages))
            }
            None => {
                let (mut n, mut more) = (2, true);
                while more && n <= max_pages {
                    let page = self.page::<P>(url_of(n)?).await?;
                    out.extend(items(page.body));
                    more = page.more;
                    n += 1;
                }
                Ok((out, more))
            }
        }
    }

    /// Login and display name of the account the token belongs to. Fails
    /// with [`ProviderError::Unauthorized`] when the provider rejects it.
    pub async fn user(&self) -> Result<(String, Option<String>), ProviderError> {
        let response = self.get(self.endpoint(&["user"], &[])?).await?;
        self.read_user(response).await.map(|(login, name, _)| (login, name))
    }

    /// `(login, name, response headers' view of the token)`.
    async fn read_user(
        &self,
        response: Response,
    ) -> Result<(String, Option<String>, (Vec<String>, Option<DateTime<Utc>>)), ProviderError> {
        match self.kind {
            GitProvider::Github => {
                // Classic tokens report their scopes; every expiring token its expiry.
                let scopes = header_str(&response, "x-oauth-scopes").map(split_scopes).unwrap_or_default();
                let expires =
                    header_str(&response, "github-authentication-token-expiration").and_then(parse_github_expiry);
                let user: GithubUser = self.json(response).await?;
                Ok((user.login, non_empty(user.name), (scopes, expires)))
            }
            GitProvider::Gitlab => {
                let user: GitlabUser = self.json(response).await?;
                Ok((user.username, non_empty(user.name), (Vec::new(), None)))
            }
        }
    }

    /// The account a personal access token belongs to, and what the
    /// provider says about that token (its scopes, when it expires).
    pub async fn account(&self) -> Result<Account, ProviderError> {
        let response = self.get(self.endpoint(&["user"], &[])?).await?;
        let (login, name, (mut scopes, mut token_expires_at)) = self.read_user(response).await?;
        if self.kind == GitProvider::Gitlab {
            // Best effort: only personal access tokens can describe themselves.
            let token = match self.get(self.endpoint(&["personal_access_tokens", "self"], &[])?).await {
                Ok(response) => response.json::<GitlabToken>().await.unwrap_or_default(),
                Err(_) => GitlabToken::default(),
            };
            scopes = token.scopes;
            token_expires_at = token.expires_at.as_deref().and_then(parse_gitlab_expiry);
        }
        Ok(Account { login, name, scopes, token_expires_at })
    }

    /// The repositories a user's token can access, most recently updated
    /// first, and whether there are more than the [`MAX_REPOSITORY_PAGES`]
    /// pages listed.
    pub async fn repositories(&self) -> Result<(Vec<GitRepository>, bool), ProviderError> {
        match self.kind {
            GitProvider::Github => {
                let (repos, truncated) = self
                    .all_pages(
                        MAX_REPOSITORY_PAGES,
                        |page| {
                            self.endpoint(
                                &["user", "repos"],
                                &[
                                    ("per_page", PER_PAGE),
                                    ("page", &page.to_string()),
                                    ("sort", "pushed"),
                                    ("direction", "desc"),
                                ],
                            )
                        },
                        |repos: Vec<GithubRepository>| repos,
                    )
                    .await?;
                Ok((repos.into_iter().map(GitRepository::from).collect(), truncated))
            }
            GitProvider::Gitlab => {
                let (projects, truncated) = self
                    .all_pages(
                        MAX_REPOSITORY_PAGES,
                        |page| {
                            self.endpoint(
                                &["projects"],
                                &[
                                    ("membership", "true"),
                                    ("order_by", "last_activity_at"),
                                    ("sort", "desc"),
                                    ("per_page", PER_PAGE),
                                    ("page", &page.to_string()),
                                ],
                            )
                        },
                        |projects: Vec<GitlabProject>| projects,
                    )
                    .await?;
                Ok((projects.into_iter().map(GitRepository::from).collect(), truncated))
            }
        }
    }

    /// The repositories the token of a GitHub App installation can access
    /// (the ones the account selected), most recently updated first.
    pub async fn installation_repositories(&self) -> Result<(Vec<GitRepository>, bool), ProviderError> {
        let (repos, truncated) = self
            .all_pages(
                MAX_REPOSITORY_PAGES,
                |page| {
                    self.endpoint(
                        &["installation", "repositories"],
                        &[("per_page", PER_PAGE), ("page", &page.to_string())],
                    )
                },
                |page: GithubInstallationRepositories| page.repositories,
            )
            .await?;
        let mut repos: Vec<GitRepository> = repos.into_iter().map(GitRepository::from).collect();
        // This listing has no order of its own.
        repos.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.full_name.cmp(&b.full_name)));
        Ok((repos, truncated))
    }

    /// Exchange the code GitHub hands back after a GitHub App was
    /// registered from a manifest for the app and its secrets (the code is
    /// the credential: no token).
    pub async fn convert_manifest(&self, code: &str) -> Result<AppRegistration, ProviderError> {
        let response =
            self.post(self.endpoint(&["app-manifests", code, "conversions"], &[])?).await.map_err(|e| match e {
                ProviderError::NotFound(_) => ProviderError::Unauthorized(format!(
                    "{} doesn't know this app registration (it expires after an hour): start again",
                    self.label()
                )),
                other => other,
            })?;
        let app: GithubAppConversion = self.json(response).await?;
        Ok(AppRegistration {
            id: app.id,
            slug: app.slug,
            url: app.html_url,
            client_id: app.client_id,
            client_secret: app.client_secret,
            private_key: app.pem,
            webhook_secret: non_empty(app.webhook_secret),
            owner: app.owner.login,
        })
    }

    /// Where the GitHub App is installed (the token is the app's JWT).
    pub async fn app_installations(&self) -> Result<Vec<Installation>, ProviderError> {
        let (installations, _) = self
            .all_pages(
                MAX_INSTALLATION_PAGES,
                |page| self.endpoint(&["app", "installations"], &[("per_page", PER_PAGE), ("page", &page.to_string())]),
                |installations: Vec<GithubInstallationWire>| installations,
            )
            .await?;
        Ok(installations.into_iter().map(Installation::from).collect())
    }

    /// One installation of the GitHub App (the token is the app's JWT).
    /// [`ProviderError::NotFound`] when it isn't one of this app's.
    pub async fn app_installation(&self, id: u64) -> Result<Installation, ProviderError> {
        let response = self.get(self.endpoint(&["app", "installations", &id.to_string()], &[])?).await?;
        Ok(Installation::from(self.json::<GithubInstallationWire>(response).await?))
    }

    /// A token of an installation of the GitHub App (the token is the
    /// app's JWT), good for an hour.
    pub async fn installation_token(&self, id: u64) -> Result<InstallationToken, ProviderError> {
        let url = self.endpoint(&["app", "installations", &id.to_string(), "access_tokens"], &[])?;
        let token: GithubInstallationTokenWire = self.json(self.post(url).await?).await?;
        Ok(InstallationToken { token: token.token, expires_at: token.expires_at })
    }

    /// The OAuth token endpoint (`<instance>/oauth/token`): exchanges an
    /// authorization code, or a refresh token, for tokens. The client
    /// secret travels in the form, not in a header.
    pub async fn oauth_token(&self, form: &[(&str, &str)]) -> Result<OAuthTokens, ProviderError> {
        let url = self.url(self.base_url, &["oauth", "token"], &[])?;
        let request = self.client()?.post(url.clone()).header(header::ACCEPT, "application/json").form(form);
        let response = request.send().await.map_err(|e| {
            ProviderError::Unavailable(format!(
                "cannot reach {} at {}: {}",
                self.label(),
                url.origin().ascii_serialization(),
                root_cause(&e)
            ))
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // Never echo the secrets that were sent.
            let mut detail = api_message(&body).map(|m| format!(": {m}")).unwrap_or_default();
            let secret = |key: &str| matches!(key, "client_secret" | "code" | "refresh_token");
            for (_, value) in form.iter().filter(|(k, v)| secret(k) && v.len() >= 6) {
                detail = detail.replace(value, "***");
            }
            let label = self.label();
            return Err(match status {
                // `invalid_grant`, `invalid_client`: what was presented is not accepted.
                StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED => {
                    ProviderError::Unauthorized(format!("{label} refused the authorization{detail}"))
                }
                StatusCode::NOT_FOUND => ProviderError::NotFound(format!("{label} answered 404 Not Found{detail}")),
                other => ProviderError::Unavailable(format!("{label} answered {other}{detail}")),
            });
        }
        let tokens: OAuthTokenWire = self.json(response).await?;
        if tokens.access_token.trim().is_empty() {
            return Err(ProviderError::Unavailable(format!("{} answered without an access token", self.label())));
        }
        let issued = tokens.created_at.and_then(|t| DateTime::from_timestamp(t, 0)).unwrap_or_else(Utc::now);
        Ok(OAuthTokens {
            access_token: tokens.access_token,
            refresh_token: non_empty(tokens.refresh_token),
            expires_at: tokens.expires_in.filter(|s| *s > 0).map(|s| issued + chrono::Duration::seconds(s)),
            scopes: tokens
                .scope
                .as_deref()
                .map(|s| s.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// wire formats (only the fields Ferry uses)

#[derive(Deserialize)]
struct GithubUser {
    login: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct GitlabUser {
    username: String,
    #[serde(default)]
    name: Option<String>,
}

/// `GET /personal_access_tokens/self`
#[derive(Default, Deserialize)]
struct GitlabToken {
    #[serde(default)]
    scopes: Vec<String>,
    /// A date (`2027-01-31`): the token stops working at midnight UTC.
    #[serde(default)]
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct GithubOwner {
    login: String,
}

#[derive(Deserialize)]
struct GithubRepository {
    id: u64,
    name: String,
    full_name: String,
    owner: GithubOwner,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    default_branch: Option<String>,
    clone_url: String,
    html_url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pushed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
}

impl From<GithubRepository> for GitRepository {
    fn from(r: GithubRepository) -> Self {
        GitRepository {
            id: r.id.to_string(),
            full_name: r.full_name,
            name: r.name,
            owner: r.owner.login,
            private: r.private,
            archived: r.archived,
            default_branch: non_empty(r.default_branch),
            clone_url: r.clone_url,
            web_url: r.html_url,
            description: non_empty(r.description),
            updated_at: r.pushed_at.or(r.updated_at),
        }
    }
}

/// `GET /installation/repositories`
#[derive(Deserialize)]
struct GithubInstallationRepositories {
    #[serde(default)]
    repositories: Vec<GithubRepository>,
}

/// `POST /app-manifests/{code}/conversions`
#[derive(Deserialize)]
struct GithubAppConversion {
    id: u64,
    slug: String,
    html_url: String,
    client_id: String,
    client_secret: String,
    pem: String,
    #[serde(default)]
    webhook_secret: Option<String>,
    owner: GithubOwner,
}

#[derive(Deserialize)]
struct GithubInstallationWire {
    id: u64,
    #[serde(default)]
    account: Option<GithubOwner>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    repository_selection: Option<String>,
    #[serde(default)]
    suspended_at: Option<String>,
}

impl From<GithubInstallationWire> for Installation {
    fn from(i: GithubInstallationWire) -> Self {
        Installation {
            id: i.id,
            account: i.account.map(|a| a.login).unwrap_or_default(),
            url: non_empty(i.html_url),
            repository_selection: non_empty(i.repository_selection),
            suspended: non_empty(i.suspended_at).is_some(),
        }
    }
}

/// `POST /app/installations/{id}/access_tokens`
#[derive(Deserialize)]
struct GithubInstallationTokenWire {
    token: String,
    expires_at: DateTime<Utc>,
}

/// `POST /oauth/token`
#[derive(Deserialize)]
struct OAuthTokenWire {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Seconds the access token lasts.
    #[serde(default)]
    expires_in: Option<i64>,
    /// When it was issued (Unix seconds).
    #[serde(default)]
    created_at: Option<i64>,
    /// Space-separated.
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct GitlabNamespace {
    full_path: String,
}

#[derive(Deserialize)]
struct GitlabProject {
    id: u64,
    path: String,
    path_with_namespace: String,
    #[serde(default)]
    namespace: Option<GitlabNamespace>,
    /// `private`, `internal` or `public`.
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    archived: bool,
    /// `null` for a project without commits.
    #[serde(default)]
    default_branch: Option<String>,
    http_url_to_repo: String,
    web_url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    last_activity_at: Option<DateTime<Utc>>,
}

impl From<GitlabProject> for GitRepository {
    fn from(p: GitlabProject) -> Self {
        let owner = match p.namespace {
            Some(namespace) => namespace.full_path,
            None => p.path_with_namespace.rsplit_once('/').map(|(owner, _)| owner.to_string()).unwrap_or_default(),
        };
        GitRepository {
            id: p.id.to_string(),
            full_name: p.path_with_namespace,
            name: p.path,
            owner,
            private: p.visibility.as_deref() != Some("public"),
            archived: p.archived,
            default_branch: non_empty(p.default_branch),
            clone_url: p.http_url_to_repo,
            web_url: p.web_url,
            description: non_empty(p.description),
            updated_at: p.last_activity_at,
        }
    }
}

// ---------------------------------------------------------------------------
// helpers

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn header_str<'r>(response: &'r Response, name: &str) -> Option<&'r str> {
    response.headers().get(name).and_then(|v| v.to_str().ok())
}

/// `repo, read:org` → `["repo", "read:org"]`.
fn split_scopes(header: &str) -> Vec<String> {
    header.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The `page` parameter of the link with relation `rel` in a `Link` header:
/// `<https://api.github.com/user/repos?page=5>; rel="last"` → 5.
fn link_page(link: &str, rel: &str) -> Option<u32> {
    link.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        let wanted =
            params.split(';').any(|p| p.trim().strip_prefix("rel=").is_some_and(|r| r.trim_matches('"') == rel));
        if !wanted {
            return None;
        }
        let url = Url::parse(target.trim().strip_prefix('<')?.strip_suffix('>')?).ok()?;
        url.query_pairs().find(|(k, _)| k == "page")?.1.parse().ok()
    })
}

/// `2026-12-01 10:30:00 UTC` (also with a numeric offset).
fn parse_github_expiry(value: &str) -> Option<DateTime<Utc>> {
    let v = value.trim();
    if let Some(naive) = v.strip_suffix("UTC").map(str::trim)
        && let Ok(t) = NaiveDateTime::parse_from_str(naive, "%Y-%m-%d %H:%M:%S")
    {
        return Some(t.and_utc());
    }
    DateTime::parse_from_str(v, "%Y-%m-%d %H:%M:%S %z").ok().map(|t| t.with_timezone(&Utc))
}

/// `2027-01-31`: midnight UTC of that day.
fn parse_gitlab_expiry(value: &str) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc())
}

/// The message of a provider's JSON error body, on one line and at most 300
/// characters: GitHub's `message`, GitLab's `message` / `error_description`
/// / `error` (plus the scopes it asks for).
fn api_message(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = ["message", "error_description", "error"].iter().find_map(|k| match json.get(*k)? {
        serde_json::Value::String(s) => Some(s.clone()),
        // GitLab also answers `{"message": {"field": ["problem"]}}`.
        other if !other.is_null() => Some(other.to_string()),
        _ => None,
    })?;
    let mut text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(scopes) = json.get("scope").and_then(serde_json::Value::as_str).filter(|s| !s.trim().is_empty()) {
        text.push_str(&format!(
            " (the token needs one of these scopes: {})",
            scopes.split_whitespace().collect::<Vec<_>>().join(", ")
        ));
    }
    if text.chars().count() > 300 {
        text = text.chars().take(300).collect::<String>() + "…";
    }
    (!text.is_empty()).then_some(text)
}

/// The innermost cause of an error (reqwest's own messages only say
/// "error sending request").
fn root_cause(e: &dyn std::error::Error) -> String {
    let mut cause = e;
    while let Some(next) = cause.source() {
        cause = next;
    }
    cause.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_roots() {
        let api = |kind, base| Provider::new(kind, base, "t").api_url();
        assert_eq!(api(GitProvider::Github, "https://github.com"), "https://api.github.com");
        assert_eq!(api(GitProvider::Github, "https://ghe.example.com"), "https://ghe.example.com/api/v3");
        assert_eq!(api(GitProvider::Gitlab, "https://gitlab.com"), "https://gitlab.com/api/v4");
        assert_eq!(api(GitProvider::Gitlab, "https://example.com/gitlab"), "https://example.com/gitlab/api/v4");
    }

    #[test]
    fn endpoints_escape_segments() {
        let gl = Provider::new(GitProvider::Gitlab, "https://example.com/gitlab", "t");
        let url = gl.endpoint(&["projects", "group/sub/app", "repository", "branches"], &[("page", "2")]).unwrap();
        assert_eq!(
            url.as_str(),
            "https://example.com/gitlab/api/v4/projects/group%2Fsub%2Fapp/repository/branches?page=2"
        );
        let gh = Provider::new(GitProvider::Github, "https://github.com", "t");
        assert_eq!(gh.endpoint(&["user"], &[]).unwrap().as_str(), "https://api.github.com/user");
        // Nothing a caller-given segment contains can leave it.
        let url = gh.endpoint(&["app-manifests", "a?b#c/../d", "conversions"], &[]).unwrap();
        assert_eq!(url.as_str(), "https://api.github.com/app-manifests/a%3Fb%23c%2F..%2Fd/conversions");
        // The OAuth token endpoint is on the instance itself, below its path.
        let token = gl.url(gl.base_url, &["oauth", "token"], &[]).unwrap();
        assert_eq!(token.as_str(), "https://example.com/gitlab/oauth/token");
    }

    #[test]
    fn link_headers() {
        let link = "<https://api.github.com/user/repos?per_page=100&page=2>; rel=\"next\", \
                    <https://api.github.com/user/repos?per_page=100&page=7>; rel=\"last\"";
        assert_eq!((link_page(link, "next"), link_page(link, "last")), (Some(2), Some(7)));
        // The last page links back only.
        let link = "<https://api.github.com/user/repos?page=6>; rel=\"prev\", <https://api.github.com/user/repos?page=1>; rel=\"first\"";
        assert_eq!((link_page(link, "next"), link_page(link, "last")), (None, None));
        assert_eq!(link_page("", "next"), None);
        assert_eq!(link_page("garbage; rel=\"next\"", "next"), None);
    }

    #[test]
    fn token_expiry_formats() {
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        assert_eq!(parse_github_expiry("2026-12-01 10:30:00 UTC"), Some(at("2026-12-01T10:30:00Z")));
        assert_eq!(parse_github_expiry("2026-12-01 12:30:00 +0200"), Some(at("2026-12-01T10:30:00Z")));
        assert_eq!(parse_github_expiry("soon"), None);
        assert_eq!(parse_gitlab_expiry("2027-01-31"), Some(at("2027-01-31T00:00:00Z")));
        assert_eq!(parse_gitlab_expiry("31/01/2027"), None);
        assert_eq!(split_scopes("repo, read:org,"), vec!["repo", "read:org"]);
        assert!(split_scopes("").is_empty());
    }

    #[test]
    fn error_messages() {
        assert_eq!(
            api_message(r#"{"message":"Bad credentials","documentation_url":"https://docs"}"#).as_deref(),
            Some("Bad credentials")
        );
        assert_eq!(
            api_message(r#"{"error":"insufficient_scope","error_description":"The request requires\nhigher privileges.","scope":"api read_api"}"#).as_deref(),
            Some("The request requires higher privileges. (the token needs one of these scopes: api, read_api)")
        );
        assert_eq!(api_message(r#"{"message":{"base":["nope"]}}"#).as_deref(), Some(r#"{"base":["nope"]}"#));
        assert_eq!(api_message(r#"{"error":"invalid_token"}"#).as_deref(), Some("invalid_token"));
        assert_eq!(api_message("<html>502 Bad Gateway</html>"), None);
        assert_eq!(api_message(r#"{"message":""}"#), None);
        let long = api_message(&format!(r#"{{"message":"{}"}}"#, "x".repeat(500))).unwrap();
        assert_eq!(long.chars().count(), 301);
        // An anonymous client masks nothing (and doesn't garble the text).
        let anonymous = Provider::anonymous(GitProvider::Github, "https://github.com");
        assert_eq!(anonymous.detail(r#"{"message":"Not Found"}"#), ": Not Found");
        let with_token = Provider::new(GitProvider::Github, "https://github.com", "tok-123456");
        assert_eq!(with_token.detail(r#"{"message":"bad tok-123456"}"#), ": bad ***");
    }

    #[test]
    fn repositories_are_mapped() {
        let gh: GithubRepository = serde_json::from_str(
            r#"{"id": 42, "name": "app", "full_name": "octocat/app", "owner": {"login": "octocat", "id": 1},
                "private": true, "archived": false, "default_branch": "main", "description": "  ",
                "clone_url": "https://github.com/octocat/app.git", "html_url": "https://github.com/octocat/app",
                "pushed_at": "2026-09-30T08:15:00Z", "updated_at": "2026-01-01T00:00:00Z", "fork": false}"#,
        )
        .unwrap();
        let repo = GitRepository::from(gh);
        assert_eq!((repo.id.as_str(), repo.full_name.as_str(), repo.owner.as_str()), ("42", "octocat/app", "octocat"));
        assert!(repo.private && !repo.archived && repo.description.is_none());
        assert_eq!(repo.default_branch.as_deref(), Some("main"));
        assert_eq!(repo.clone_url, "https://github.com/octocat/app.git");
        assert_eq!(repo.updated_at.unwrap().to_rfc3339(), "2026-09-30T08:15:00+00:00");

        let gl: GitlabProject = serde_json::from_str(
            r#"{"id": 7, "path": "api", "name": "API", "path_with_namespace": "acme/platform/api",
                "namespace": {"id": 3, "full_path": "acme/platform", "kind": "group"}, "visibility": "internal",
                "archived": true, "default_branch": null, "description": "The API",
                "http_url_to_repo": "https://gitlab.com/acme/platform/api.git",
                "web_url": "https://gitlab.com/acme/platform/api", "last_activity_at": "2026-09-29T10:00:00.123Z"}"#,
        )
        .unwrap();
        let repo = GitRepository::from(gl);
        assert_eq!(
            (repo.full_name.as_str(), repo.name.as_str(), repo.owner.as_str()),
            ("acme/platform/api", "api", "acme/platform")
        );
        // `internal` isn't public; an empty project has no default branch.
        assert!(repo.private && repo.archived && repo.default_branch.is_none());
        assert_eq!(repo.description.as_deref(), Some("The API"));
        assert_eq!(repo.clone_url, "https://gitlab.com/acme/platform/api.git");
    }

    #[test]
    fn github_app_answers_are_mapped() {
        let installation: GithubInstallationWire = serde_json::from_str(
            r#"{"id": 99, "account": {"login": "octocat", "type": "User"}, "repository_selection": "selected",
                "html_url": "https://github.com/settings/installations/99", "suspended_at": null, "app_slug": "x"}"#,
        )
        .unwrap();
        let installation = Installation::from(installation);
        assert_eq!((installation.id, installation.account.as_str(), installation.suspended), (99, "octocat", false));
        assert_eq!(installation.repository_selection.as_deref(), Some("selected"));
        assert_eq!(installation.url.as_deref(), Some("https://github.com/settings/installations/99"));
        let suspended: GithubInstallationWire =
            serde_json::from_str(r#"{"id": 1, "suspended_at": "2026-09-01T00:00:00Z"}"#).unwrap();
        assert!(Installation::from(suspended).suspended);
        let page: GithubInstallationRepositories =
            serde_json::from_str(r#"{"total_count": 0, "repository_selection": "all", "repositories": []}"#).unwrap();
        assert!(page.repositories.is_empty());
    }
}
