//! Clients of the git providers' REST APIs (GitHub, GitLab), for git
//! connections: who a token belongs to, which repositories it can read and
//! their branches.
//!
//! A token only ever travels in the `Authorization` header of requests to
//! its own provider instance; it never appears in an error or a log line.

use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use ferry_core::dto::{GitBranch, GitRepository};
use ferry_core::{GitConnection, GitProvider};
use futures::StreamExt;
use reqwest::{Response, StatusCode, Url, header};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Repositories are listed 100 per page, up to this many pages.
pub(crate) const MAX_REPOSITORY_PAGES: u32 = 10;
/// Branches are listed 100 per page, up to this many pages.
pub(crate) const MAX_BRANCH_PAGES: u32 = 5;
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

/// Why a provider request failed. Messages name the provider, never the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderError {
    /// The provider rejected the token (mistyped, expired, revoked).
    Unauthorized(String),
    /// The token is valid but may not do this (a missing scope or permission).
    Forbidden(String),
    /// No such repository (or the token can't see it).
    NotFound(String),
    /// Too many requests with this token.
    RateLimited(String),
    /// The provider can't be reached, or answered something unexpected.
    Unavailable(String),
}

/// The account a token belongs to, and what the provider says about the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    pub login: String,
    pub name: Option<String>,
    pub scopes: Vec<String>,
    pub token_expires_at: Option<DateTime<Utc>>,
}

/// One instance of a provider, seen through one token.
pub(crate) struct Provider<'a> {
    kind: GitProvider,
    /// Web URL of the instance (normalized: no trailing slash).
    base_url: &'a str,
    token: &'a str,
}

/// One page of a listing.
struct Page<T> {
    items: Vec<T>,
    /// Number of the last page, when the provider says.
    last: Option<u32>,
    /// There is a page after this one.
    more: bool,
}

impl<'a> Provider<'a> {
    pub(crate) fn new(kind: GitProvider, base_url: &'a str, token: &'a str) -> Self {
        Provider { kind, base_url, token }
    }

    pub(crate) fn of(connection: &'a GitConnection) -> Self {
        Provider::new(connection.provider, &connection.base_url, &connection.token)
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

    /// `<api>/<segments...>?<query>`; a segment may contain `/` (it is escaped).
    fn endpoint(&self, segments: &[&str], query: &[(&str, &str)]) -> Result<Url, ProviderError> {
        let invalid = || ProviderError::Unavailable(format!("invalid {} address {}", self.label(), self.base_url));
        let mut url = Url::parse(&self.api_url()).map_err(|_| invalid())?;
        url.path_segments_mut().map_err(|_| invalid())?.pop_if_empty().extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    async fn get(&self, url: Url) -> Result<Response, ProviderError> {
        let http = HTTP.as_ref().map_err(|e| {
            ProviderError::Unavailable(format!("cannot set up the HTTP client for {}: {e}", self.label()))
        })?;
        let request = http.get(url.clone()).bearer_auth(self.token);
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

    /// The error of a non-2xx answer.
    async fn failure(&self, response: Response) -> ProviderError {
        let status = response.status();
        let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && (header_str(&response, "x-ratelimit-remaining") == Some("0")
                    || response.headers().contains_key(header::RETRY_AFTER)));
        let body = response.text().await.unwrap_or_default();
        let detail = match api_message(&body) {
            // Providers don't echo tokens; mask it anyway.
            Some(m) => format!(": {}", m.replace(self.token, "***")),
            None => String::new(),
        };
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

    async fn page<T: DeserializeOwned>(&self, url: Url) -> Result<Page<T>, ProviderError> {
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
        Ok(Page { items: self.json(response).await?, last, more })
    }

    /// Every item of a paginated listing, up to `max_pages` pages, in the
    /// provider's order. The flag says whether pages were left out.
    async fn all_pages<T, F>(&self, max_pages: u32, url_of: F) -> Result<(Vec<T>, bool), ProviderError>
    where
        T: DeserializeOwned,
        F: Fn(u32) -> Result<Url, ProviderError>,
    {
        let first = self.page::<T>(url_of(1)?).await?;
        let mut items = first.items;
        if !first.more {
            return Ok((items, false));
        }
        let url_of = &url_of;
        match first.last {
            // The number of pages is known: fetch the rest a few at a time.
            Some(last) => {
                let rest = futures::stream::iter(2..=last.min(max_pages))
                    .map(|n| async move { self.page::<T>(url_of(n)?).await })
                    .buffered(CONCURRENT_PAGES);
                let mut rest = std::pin::pin!(rest);
                while let Some(page) = rest.next().await {
                    items.extend(page?.items);
                }
                Ok((items, last > max_pages))
            }
            None => {
                let (mut n, mut more) = (2, true);
                while more && n <= max_pages {
                    let page = self.page::<T>(url_of(n)?).await?;
                    items.extend(page.items);
                    more = page.more;
                    n += 1;
                }
                Ok((items, more))
            }
        }
    }

    /// The account the token belongs to. Fails with
    /// [`ProviderError::Unauthorized`] when the provider rejects the token.
    pub(crate) async fn account(&self) -> Result<Account, ProviderError> {
        let response = self.get(self.endpoint(&["user"], &[])?).await?;
        match self.kind {
            GitProvider::Github => {
                // Classic tokens report their scopes; every expiring token its expiry.
                let scopes = header_str(&response, "x-oauth-scopes").map(split_scopes).unwrap_or_default();
                let token_expires_at =
                    header_str(&response, "github-authentication-token-expiration").and_then(parse_github_expiry);
                let user: GithubUser = self.json(response).await?;
                Ok(Account { login: user.login, name: non_empty(user.name), scopes, token_expires_at })
            }
            GitProvider::Gitlab => {
                let user: GitlabUser = self.json(response).await?;
                // Best effort: only personal access tokens can describe themselves.
                let token = match self.get(self.endpoint(&["personal_access_tokens", "self"], &[])?).await {
                    Ok(response) => response.json::<GitlabToken>().await.unwrap_or_default(),
                    Err(_) => GitlabToken::default(),
                };
                Ok(Account {
                    login: user.username,
                    name: non_empty(user.name),
                    scopes: token.scopes,
                    token_expires_at: token.expires_at.as_deref().and_then(parse_gitlab_expiry),
                })
            }
        }
    }

    /// The repositories the token can access, most recently updated first,
    /// and whether there are more than the [`MAX_REPOSITORY_PAGES`] pages listed.
    pub(crate) async fn repositories(&self) -> Result<(Vec<GitRepository>, bool), ProviderError> {
        match self.kind {
            GitProvider::Github => {
                let (repos, truncated) = self
                    .all_pages::<GithubRepository, _>(MAX_REPOSITORY_PAGES, |page| {
                        self.endpoint(
                            &["user", "repos"],
                            &[
                                ("per_page", PER_PAGE),
                                ("page", &page.to_string()),
                                ("sort", "pushed"),
                                ("direction", "desc"),
                            ],
                        )
                    })
                    .await?;
                Ok((repos.into_iter().map(GitRepository::from).collect(), truncated))
            }
            GitProvider::Gitlab => {
                let (projects, truncated) = self
                    .all_pages::<GitlabProject, _>(MAX_REPOSITORY_PAGES, |page| {
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
                    })
                    .await?;
                Ok((projects.into_iter().map(GitRepository::from).collect(), truncated))
            }
        }
    }

    /// The branches of `repository` (its full name: `owner/name`), up to
    /// [`MAX_BRANCH_PAGES`] pages.
    pub(crate) async fn branches(&self, repository: &str) -> Result<Vec<GitBranch>, ProviderError> {
        let paging = |page: u32| [("per_page", PER_PAGE.to_string()), ("page", page.to_string())];
        let (branches, _) = self
            .all_pages::<ProviderBranch, _>(MAX_BRANCH_PAGES, |page| {
                let query = paging(page);
                let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
                match self.kind {
                    GitProvider::Github => {
                        let mut segments = vec!["repos"];
                        segments.extend(repository.split('/'));
                        segments.push("branches");
                        self.endpoint(&segments, &query)
                    }
                    // The whole path is one (escaped) segment.
                    GitProvider::Gitlab => self.endpoint(&["projects", repository, "repository", "branches"], &query),
                }
            })
            .await
            .map_err(|e| match e {
                ProviderError::NotFound(_) => ProviderError::NotFound(format!(
                    "repository '{repository}' not found on {} (or the token can't access it)",
                    self.label()
                )),
                other => other,
            })?;
        Ok(branches.into_iter().map(|b| GitBranch { name: b.name, protected: b.protected }).collect())
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

/// A branch, as both providers list it.
#[derive(Deserialize)]
struct ProviderBranch {
    name: String,
    #[serde(default)]
    protected: bool,
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
        // Nothing a repository name contains can leave its segment.
        let url = gh.endpoint(&["repos", "a?b#c", "..", "branches"], &[]).unwrap();
        assert_eq!(url.as_str(), "https://api.github.com/repos/a%3Fb%23c/branches");
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
}
