//! Git connections: authorizing GitHub / GitLab accounts in the browser (a
//! GitHub App, an OAuth application) or with an access token, listing their
//! repositories, reading a repository's branches, and services cloned with
//! the connection that serves their repository — against a fake provider on
//! a local socket.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use base64::Engine as _;
use chrono::Utc;
use common::TestApp;
use ferry_core::{GitAuth, GitConnection, GitProvider};
use http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

const GH_TOKEN: &str = "ghp_validTokenOfOctocat0001";
const GH_TOKEN_2: &str = "ghp_renewedTokenOfOctocat9XYZ";
const GL_TOKEN: &str = "glpat-validTokenOfTanuki-7Qr";
const LIMITED: &str = "ghp_rateLimitedToken";
const NO_SCOPE: &str = "glpat-withoutTheApiScope";

/// Where the providers send the browser back (the dashboard's callback page).
const REDIRECT: &str = "http://localhost:7878/git/callback";
/// The GitHub App the fake registers.
const APP_ID: u64 = 4242;
const APP_SLUG: &str = "ferry-test-app";
/// The OAuth application "created" on the fake GitLab.
const GL_APP_ID: &str = "0a1b2c3d4e5f-application";
const GL_APP_KEY: &str = "gloas-applicationKeyOfTheFake";
const GL_OTHER_KEY: &str = "gloas-notTheRightOne";

/// A fake GitHub Enterprise Server (`/api/v3`, app pages) and GitLab
/// (`/api/v4`, `/oauth/token`) on one address, which also serves one git
/// repository over http to requests that carry a known token.
struct Fake {
    /// Bearer token → login (personal, OAuth and installation tokens).
    tokens: Mutex<HashMap<String, String>>,
    /// How many repositories / projects every account sees.
    repositories: AtomicUsize,
    /// Listings say how many pages they have (GitHub: `rel="last"`, GitLab: `x-total-pages`).
    announce_pages: AtomicBool,
    /// `METHOD path?query` of every request, in order.
    requests: Mutex<Vec<String>>,
    /// Manifest codes GitHub would hand back after registering an app → its owner.
    manifest_codes: Mutex<HashMap<String, String>>,
    /// Installations of the app, as GitHub describes them.
    installations: Mutex<Vec<Value>>,
    /// Authorization codes GitLab would hand back → the account that authorized.
    oauth_codes: Mutex<HashMap<String, String>>,
    /// Refresh tokens that still work → login.
    refresh_tokens: Mutex<HashMap<String, String>>,
    /// Tokens issued so far (they are numbered).
    issued: AtomicUsize,
    /// The bare repository served at `/<anything>.git/`.
    repo: Mutex<Option<PathBuf>>,
    /// `user:password` of the git requests that were let in.
    git_logins: Mutex<Vec<String>>,
}

impl Fake {
    fn login(&self, headers: &HeaderMap) -> Option<String> {
        let token = headers.get("authorization")?.to_str().ok()?.strip_prefix("Bearer ")?;
        self.tokens.lock().unwrap().get(token).cloned()
    }

    fn revoke(&self, token: &str) {
        self.tokens.lock().unwrap().remove(token);
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn requests_to(&self, path: &str) -> usize {
        self.requests().iter().filter(|r| r.contains(path)).count()
    }

    /// GitHub registered the app for `owner` and sends the browser back with this code.
    fn registered(&self, code: &str, owner: &str) {
        self.manifest_codes.lock().unwrap().insert(code.into(), owner.into());
    }

    /// `account` installed the app.
    fn installed(&self, id: u64, account: &str, selection: &str) {
        let mut all = self.installations.lock().unwrap();
        all.retain(|i| i["id"] != id);
        all.push(json!({
            "id": id,
            "account": {"login": account, "id": 1, "type": "User"},
            "repository_selection": selection,
            "html_url": format!("https://github.example/settings/installations/{id}"),
            "app_id": APP_ID,
            "app_slug": APP_SLUG,
            "suspended_at": null,
        }));
    }

    fn uninstalled(&self, id: u64) {
        self.installations.lock().unwrap().retain(|i| i["id"] != id);
    }

    /// `login` authorized the OAuth application: GitLab sends the browser back with this code.
    fn authorized(&self, code: &str, login: &str) {
        self.oauth_codes.lock().unwrap().insert(code.into(), login.into());
    }

    /// New OAuth tokens for `login` (those it had stop working).
    fn issue(&self, login: &str) -> Value {
        let n = self.issued.fetch_add(1, Ordering::SeqCst) + 1;
        let (access, refresh) = (format!("gl-access-{n}-{login}"), format!("gl-refresh-{n}-{login}"));
        let mut tokens = self.tokens.lock().unwrap();
        tokens.retain(|t, l| !(l == login && t.starts_with("gl-access-")));
        tokens.insert(access.clone(), login.into());
        self.refresh_tokens.lock().unwrap().insert(refresh.clone(), login.into());
        json!({
            "access_token": access, "token_type": "Bearer", "expires_in": 7200, "refresh_token": refresh,
            "scope": "read_api read_repository", "created_at": Utc::now().timestamp(),
        })
    }
}

type FakeState = State<Arc<Fake>>;

fn own_url(headers: &HeaderMap) -> String {
    format!("http://{}", headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("127.0.0.1"))
}

fn page_of(q: &HashMap<String, String>) -> usize {
    assert_eq!(q.get("per_page").map(String::as_str), Some("100"), "{q:?}");
    q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1)
}

/// 1-based item numbers of `page`, and the number of pages.
fn paginate(total: usize, page: usize) -> (std::ops::Range<usize>, usize) {
    let pages = total.div_ceil(100).max(1);
    ((page - 1) * 100 + 1..(page * 100).min(total) + 1, pages)
}

fn github_denied() -> Response {
    let body = json!({"message": "Bad credentials", "documentation_url": "https://docs.github.com/rest"});
    (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
}

fn github_not_found() -> Response {
    (StatusCode::NOT_FOUND, axum::Json(json!({"message": "Not Found"}))).into_response()
}

async fn gh_user(State(fake): FakeState, headers: HeaderMap) -> Response {
    let Some(login) = fake.login(&headers) else { return github_denied() };
    if login == "limited" {
        let body = json!({"message": "API rate limit exceeded for user ID 1."});
        return (StatusCode::FORBIDDEN, [("x-ratelimit-remaining", "0")], axum::Json(body)).into_response();
    }
    let extra =
        [("x-oauth-scopes", "repo, read:org"), ("github-authentication-token-expiration", "2027-01-01 00:00:00 UTC")];
    (extra, axum::Json(json!({"login": login, "id": 1, "name": "The Octocat"}))).into_response()
}

fn github_repository(base: &str, login: &str, i: usize) -> Value {
    json!({
        "id": 1000 + i,
        "name": format!("repo-{i:04}"),
        "full_name": format!("{login}/repo-{i:04}"),
        "owner": {"login": login, "id": 1},
        "private": i % 2 == 1,
        "archived": i == 3,
        "fork": false,
        "default_branch": if i == 2 { "trunk" } else { "main" },
        "description": if i == 1 { json!("The first one") } else { Value::Null },
        "clone_url": format!("{base}/{login}/repo-{i:04}.git"),
        "html_url": format!("{base}/{login}/repo-{i:04}"),
        "pushed_at": "2026-09-30T08:15:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
    })
}

async fn gh_repos(State(fake): FakeState, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(login) = fake.login(&headers) else { return github_denied() };
    assert_eq!(q.get("sort").map(String::as_str), Some("pushed"), "{q:?}");
    let (base, page) = (own_url(&headers), page_of(&q));
    let (items, pages) = paginate(fake.repositories.load(Ordering::SeqCst), page);
    let repos: Vec<Value> = items.map(|i| github_repository(&base, &login, i)).collect();
    let mut link = Vec::new();
    if page < pages {
        link.push(format!("<{base}/api/v3/user/repos?per_page=100&page={}>; rel=\"next\"", page + 1));
        if fake.announce_pages.load(Ordering::SeqCst) {
            link.push(format!("<{base}/api/v3/user/repos?per_page=100&page={pages}>; rel=\"last\""));
        }
    }
    ([("link", link.join(", "))], axum::Json(repos)).into_response()
}

// --- the GitHub App -------------------------------------------------------

/// An RSA key for the fake app, made once with the `openssl` CLI. `None`
/// without `openssl` (the app tests are skipped).
fn app_key() -> Option<&'static str> {
    static KEY: OnceLock<Option<String>> = OnceLock::new();
    KEY.get_or_init(|| {
        let out = std::process::Command::new("openssl").args(["genrsa", "2048"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    })
    .as_deref()
}

async fn gh_manifest(State(fake): FakeState, headers: HeaderMap, Path(code): Path<String>) -> Response {
    // The code is the credential: nothing else is sent.
    assert!(headers.get("authorization").is_none(), "a manifest conversion carries no token");
    let Some(owner) = fake.manifest_codes.lock().unwrap().remove(&code) else { return github_not_found() };
    let base = own_url(&headers);
    let app = json!({
        "id": APP_ID, "slug": APP_SLUG, "name": APP_SLUG, "node_id": "A_kwDOtest",
        "owner": {"login": owner, "id": 1, "type": "User"},
        "html_url": format!("{base}/apps/{APP_SLUG}"),
        "client_id": "Iv1.clientIdOfTheFakeApp",
        "client_secret": format!("{}{}", "app-client-", "s3cretOfTheFake"),
        "webhook_secret": null,
        "pem": app_key().expect("openssl"),
        "permissions": {"contents": "read", "metadata": "read"},
        "events": [],
    });
    (StatusCode::CREATED, axum::Json(app)).into_response()
}

/// Whether the request authenticates as the app: a JWT issued by it, valid now.
fn is_app(headers: &HeaderMap) -> bool {
    let Some(jwt) = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let parts: Vec<&str> = jwt.split('.').collect();
    let decode = |s: &str| -> Option<Value> {
        serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()?).ok()
    };
    let (Some(header), Some(claims)) = (parts.first().and_then(|p| decode(p)), parts.get(1).and_then(|p| decode(p)))
    else {
        return false;
    };
    let now = Utc::now().timestamp();
    parts.len() == 3
        && header["alg"] == "RS256"
        // The app's id, as a string.
        && claims["iss"].as_str().and_then(|iss| iss.parse().ok()) == Some(APP_ID)
        && claims["iat"].as_i64().is_some_and(|iat| iat <= now)
        && claims["exp"].as_i64().is_some_and(|exp| exp > now && exp <= now + 600)
}

async fn gh_installations(State(fake): FakeState, headers: HeaderMap) -> Response {
    if !is_app(&headers) {
        return github_denied();
    }
    axum::Json(Value::Array(fake.installations.lock().unwrap().clone())).into_response()
}

async fn gh_installation(State(fake): FakeState, headers: HeaderMap, Path(id): Path<u64>) -> Response {
    if !is_app(&headers) {
        return github_denied();
    }
    match fake.installations.lock().unwrap().iter().find(|i| i["id"] == id) {
        Some(installation) => axum::Json(installation.clone()).into_response(),
        None => github_not_found(),
    }
}

async fn gh_installation_token(State(fake): FakeState, headers: HeaderMap, Path(id): Path<u64>) -> Response {
    if !is_app(&headers) {
        return github_denied();
    }
    let Some(account) = fake
        .installations
        .lock()
        .unwrap()
        .iter()
        .find(|i| i["id"] == id)
        .map(|i| i["account"]["login"].as_str().unwrap().to_string())
    else {
        return github_not_found();
    };
    let n = fake.issued.fetch_add(1, Ordering::SeqCst) + 1;
    let token = format!("ghs_installation{id}n{n}");
    fake.tokens.lock().unwrap().insert(token.clone(), account);
    let body = json!({
        "token": token,
        "expires_at": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "permissions": {"contents": "read", "metadata": "read"},
        "repository_selection": "selected",
    });
    (StatusCode::CREATED, axum::Json(body)).into_response()
}

/// What an installation may read: three repositories, oldest first (this
/// listing has no order of its own).
async fn gh_installation_repos(State(fake): FakeState, headers: HeaderMap) -> Response {
    let Some(login) = fake.login(&headers) else { return github_denied() };
    let base = own_url(&headers);
    let repos: Vec<Value> = (1..=3)
        .map(|i| {
            let mut repo = github_repository(&base, &login, i);
            repo["pushed_at"] = json!(format!("2026-09-{:02}T08:15:00Z", 10 + i));
            repo
        })
        .collect();
    axum::Json(json!({"total_count": 3, "repository_selection": "selected", "repositories": repos})).into_response()
}

// --- GitLab ----------------------------------------------------------------

fn gitlab_denied() -> Response {
    (StatusCode::UNAUTHORIZED, axum::Json(json!({"message": "401 Unauthorized"}))).into_response()
}

async fn gl_user(State(fake): FakeState, headers: HeaderMap) -> Response {
    let Some(login) = fake.login(&headers) else { return gitlab_denied() };
    if login == "noscope" {
        let body = json!({
            "error": "insufficient_scope",
            "error_description": "The request requires higher privileges than provided by the access token.",
            "scope": "api read_api",
        });
        return (StatusCode::FORBIDDEN, axum::Json(body)).into_response();
    }
    axum::Json(json!({"id": 7, "username": login, "name": "Tanuki", "state": "active"})).into_response()
}

async fn gl_token(State(fake): FakeState, headers: HeaderMap) -> Response {
    if fake.login(&headers).is_none() {
        return gitlab_denied();
    }
    let body = json!({"id": 3, "name": "ferry", "scopes": ["read_api", "read_repository"], "expires_at": "2027-03-01"});
    axum::Json(body).into_response()
}

async fn gl_projects(State(fake): FakeState, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    if fake.login(&headers).is_none() {
        return gitlab_denied();
    }
    assert_eq!(q.get("membership").map(String::as_str), Some("true"), "{q:?}");
    let (base, page) = (own_url(&headers), page_of(&q));
    let (items, pages) = paginate(fake.repositories.load(Ordering::SeqCst), page);
    let projects: Vec<Value> = items
        .map(|i| {
            let visibility = ["private", "internal", "public"][i % 3];
            json!({
                "id": 500 + i,
                "name": format!("App {i}"),
                "path": format!("app-{i:04}"),
                "path_with_namespace": format!("acme/platform/app-{i:04}"),
                "namespace": {"id": 9, "full_path": "acme/platform", "kind": "group"},
                "visibility": visibility,
                "archived": false,
                "default_branch": if i == 2 { Value::Null } else { json!("main") },
                "description": "",
                "http_url_to_repo": format!("{base}/acme/platform/app-{i:04}.git"),
                "web_url": format!("{base}/acme/platform/app-{i:04}"),
                "last_activity_at": "2026-09-29T10:00:00.123Z",
            })
        })
        .collect();
    let next = if page < pages { (page + 1).to_string() } else { String::new() };
    let mut extra = vec![("x-next-page", next), ("x-page", page.to_string())];
    if fake.announce_pages.load(Ordering::SeqCst) {
        extra.push(("x-total-pages", pages.to_string()));
    }
    let mut response = axum::Json(projects).into_response();
    for (name, value) in extra {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}

/// `POST /oauth/token`: the application's id and key come in the form.
async fn gl_oauth_token(State(fake): FakeState, Form(form): Form<HashMap<String, String>>) -> Response {
    let field = |name: &str| form.get(name).map(String::as_str).unwrap_or_default();
    let refused = |status: StatusCode, error: &str, description: &str| {
        (status, axum::Json(json!({"error": error, "error_description": description}))).into_response()
    };
    if (field("client_id"), field("client_secret")) != (GL_APP_ID, GL_APP_KEY) {
        return refused(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.",
        );
    }
    let invalid_grant = || {
        refused(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The provided authorization grant is invalid, expired, revoked, does not match the redirection URI used in the authorization request, or was issued to another client.",
        )
    };
    let login = match field("grant_type") {
        "authorization_code" if field("redirect_uri") == REDIRECT => {
            fake.oauth_codes.lock().unwrap().remove(field("code"))
        }
        // A refresh token works once.
        "refresh_token" => fake.refresh_tokens.lock().unwrap().remove(field("refresh_token")),
        _ => None,
    };
    match login {
        Some(login) => axum::Json(fake.issue(&login)).into_response(),
        None => invalid_grant(),
    }
}

// --- git over http ----------------------------------------------------------

/// `/<owner>/<name>.git/<file>`: the bare repository's files (the "dumb"
/// protocol), to requests whose Basic password is a known token.
async fn git_http(State(fake): FakeState, headers: HeaderMap, uri: http::Uri) -> Response {
    let Some((_, file)) = uri.path().split_once(".git/") else { return StatusCode::NOT_FOUND.into_response() };
    let login = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .and_then(|b| String::from_utf8(b).ok());
    let known = login
        .as_deref()
        .and_then(|l| l.split_once(':'))
        .is_some_and(|(_, password)| fake.tokens.lock().unwrap().contains_key(password));
    if !known {
        return (StatusCode::UNAUTHORIZED, [("www-authenticate", "Basic realm=\"git\"")]).into_response();
    }
    fake.git_logins.lock().unwrap().push(login.unwrap_or_default());
    let repo = fake.repo.lock().unwrap().clone();
    match repo.and_then(|repo| std::fs::read(repo.join(file)).ok()) {
        Some(body) => ([("content-type", "application/octet-stream")], body).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=Ferry Test", "-c", "user.email=test@ferry.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn have(program: &str) -> bool {
    std::process::Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// A repository with the branches `main` (the default), `dev` and
/// `feature/login`; returns its directory and a bare copy servable over
/// "dumb" http.
fn sample_repository(dir: &std::path::Path) -> (PathBuf, PathBuf) {
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("README.md"), "hello").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "first"]);
    git(&work, &["branch", "feature/login"]);
    git(&work, &["branch", "dev"]);
    git(dir, &["clone", "-q", "--bare", "work", "served.git"]);
    let bare = dir.join("served.git");
    git(&bare, &["update-server-info"]);
    (work, bare)
}

async fn record(State(fake): FakeState, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let target = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
    fake.requests.lock().unwrap().push(format!("{} {target}", req.method()));
    next.run(req).await
}

/// Start the fake; returns it with its web URL (the `base_url` of connections).
async fn fake_provider() -> (Arc<Fake>, String) {
    let tokens = [
        (GH_TOKEN, "octocat"),
        (GH_TOKEN_2, "octocat"),
        (GL_TOKEN, "tanuki"),
        (LIMITED, "limited"),
        (NO_SCOPE, "noscope"),
    ];
    let fake = Arc::new(Fake {
        tokens: Mutex::new(tokens.iter().map(|(t, l)| (t.to_string(), l.to_string())).collect()),
        repositories: AtomicUsize::new(3),
        announce_pages: AtomicBool::new(true),
        requests: Mutex::default(),
        manifest_codes: Mutex::default(),
        installations: Mutex::default(),
        oauth_codes: Mutex::default(),
        refresh_tokens: Mutex::default(),
        issued: AtomicUsize::new(0),
        repo: Mutex::default(),
        git_logins: Mutex::default(),
    });
    let router = Router::new()
        .route("/api/v3/user", get(gh_user))
        .route("/api/v3/user/repos", get(gh_repos))
        .route("/api/v3/app-manifests/{code}/conversions", post(gh_manifest))
        .route("/api/v3/app/installations", get(gh_installations))
        .route("/api/v3/app/installations/{id}", get(gh_installation))
        .route("/api/v3/app/installations/{id}/access_tokens", post(gh_installation_token))
        .route("/api/v3/installation/repositories", get(gh_installation_repos))
        .route("/api/v4/user", get(gl_user))
        .route("/api/v4/personal_access_tokens/self", get(gl_token))
        .route("/api/v4/projects", get(gl_projects))
        .route("/oauth/token", post(gl_oauth_token))
        .fallback(git_http)
        .layer(axum::middleware::from_fn_with_state(fake.clone(), record))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (fake, base)
}

/// Connect an account with a token through the API; returns its view.
async fn connect(app: &TestApp, provider: &str, base: &str, token: &str) -> Value {
    let r = app.post("/api/v1/git/connections", json!({"provider": provider, "token": token, "base_url": base})).await;
    assert!(r.status.is_success(), "{} {}", r.status, r.text());
    r.json()
}

/// The `state` parameter of a provider page.
fn state_of(url: &str) -> String {
    let (_, query) = url.split_once('?').unwrap_or_else(|| panic!("no query string in {url}"));
    form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| panic!("no state in {url}"))
}

#[tokio::test]
async fn connecting_with_a_token_asks_the_provider_and_never_returns_it() {
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    assert_eq!(app.get("/api/v1/git/connections").await.json(), json!([]));

    let r =
        app.post("/api/v1/git/connections", json!({"provider": "github", "token": GH_TOKEN, "base_url": base})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let v = r.json();
    let id = v["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("git-") && id.len() == 24, "{id}");
    assert_eq!((&v["provider"], &v["auth"], &v["status"]), (&json!("github"), &json!("token"), &json!("connected")));
    assert_eq!(v["base_url"], base);
    assert_eq!((&v["account"], &v["account_name"]), (&json!("octocat"), &json!("The Octocat")));
    assert_eq!(v["scopes"], json!(["repo", "read:org"]));
    assert_eq!(v["token_expires_at"], "2027-01-01T00:00:00Z");
    assert_eq!(v["services"], json!([]));
    // Nothing of an app: this account pasted a token.
    for key in ["client_id", "app_slug", "app_url", "manage_url", "repository_selection"] {
        assert_eq!(v[key], Value::Null, "{key}");
    }
    // Only a hint of the token ever comes back.
    assert_eq!(v["token_hint"], "…0001");
    assert!(v.get("token").is_none() && !r.text().contains("ghp_"), "{}", r.text());
    assert_eq!(app.store.require_git_connection(&id).await.unwrap().token, GH_TOKEN);
    assert_eq!(fake.requests(), vec!["GET /api/v3/user"]);

    let listed = app.get("/api/v1/git/connections").await;
    assert_eq!(listed.json(), json!([v.clone()]));
    assert!(!listed.text().contains("ghp_"));
    assert_eq!(app.get(&format!("/api/v1/git/connections/{id}")).await.json(), v);
    let missing = app.get("/api/v1/git/connections/git-00000000000000000000").await;
    assert_eq!((missing.status, missing.code().as_str()), (StatusCode::NOT_FOUND, "not_found"));

    // Connecting the same account again replaces its token (surrounding
    // spaces of a pasted token are dropped).
    let r = app
        .post(
            "/api/v1/git/connections",
            json!({"provider": "github", "token": format!("  {GH_TOKEN_2}\n"), "base_url": format!("{base}/")}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let again = r.json();
    assert_eq!((&again["id"], &again["token_hint"]), (&json!(id), &json!("…9XYZ")));
    assert_eq!(app.store.require_git_connection(&id).await.unwrap().token, GH_TOKEN_2);
    assert_eq!(app.get("/api/v1/git/connections").await.json().as_array().unwrap().len(), 1);

    // Another provider on the same address is another connection.
    let gl = connect(&app, "gitlab", &base, GL_TOKEN).await;
    assert_eq!(
        (&gl["provider"], &gl["account"], &gl["account_name"]),
        (&json!("gitlab"), &json!("tanuki"), &json!("Tanuki"))
    );
    assert_eq!(gl["scopes"], json!(["read_api", "read_repository"]));
    assert_eq!(gl["token_expires_at"], "2027-03-01T00:00:00Z");
    assert_eq!(app.get("/api/v1/git/connections").await.json().as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn tokens_the_provider_rejects_are_not_stored() {
    let app = TestApp::new().await;
    let (_fake, base) = fake_provider().await;
    let post = |body: Value| app.post("/api/v1/git/connections", body);
    let rejected = "git_authorization_rejected";

    // Never a 401: that would mean "wrong Ferry token" to API clients.
    let r = post(json!({"provider": "github", "token": "ghp_notAValidToken", "base_url": base})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, rejected), "{}", r.text());
    let message = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert_eq!(
        message,
        "GitHub rejected the token: Bad credentials: check that it was copied completely and has not expired"
    );

    let r = post(json!({"provider": "gitlab", "token": NO_SCOPE, "base_url": base})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, rejected), "{}", r.text());
    assert!(r.text().contains("the token needs one of these scopes: api, read_api"), "{}", r.text());

    let r = post(json!({"provider": "github", "token": LIMITED, "base_url": base})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_GATEWAY, "git_provider_unavailable"), "{}", r.text());
    assert!(r.text().contains("rate limiting"), "{}", r.text());

    // Not a provider at all: nothing listens / no such API.
    let r = post(json!({"provider": "gitlab", "token": GL_TOKEN, "base_url": "http://127.0.0.1:9"})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_GATEWAY, "git_provider_unavailable"), "{}", r.text());
    assert!(r.text().contains("cannot reach GitLab at http://127.0.0.1:9: "), "{}", r.text());
    let r = post(json!({"provider": "gitlab", "token": GL_TOKEN, "base_url": format!("{base}/nothing-here")})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(r.text().contains("doesn't look like a GitLab instance"), "{}", r.text());

    // Malformed requests don't reach the provider.
    for (body, word) in [
        (json!({"provider": "github", "token": "  "}), "empty"),
        (json!({"provider": "github", "token": "two words"}), "invalid access token"),
        (json!({"provider": "bitbucket", "token": "x"}), "GitProvider"),
        (json!({"provider": "github"}), "token"),
        (json!({"provider": "github", "token": "x", "base_url": "ftp://ghe.example.com"}), "base_url"),
        (json!({"provider": "github", "token": "x", "base_url": "https://u:p@ghe.example.com"}), "base_url"),
        (json!({"provider": "github", "token": "x", "scopes": ["repo"]}), "unknown field"),
    ] {
        let r = post(body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
        assert!(r.text().contains(word), "{body}: {}", r.text());
    }
    assert!(app.store.list_git_connections().await.unwrap().is_empty());

    // Like the rest of the API, every git route needs the Ferry token.
    for (method, path) in [
        ("POST", "/api/v1/git/connections"),
        ("POST", "/api/v1/git/authorize"),
        ("POST", "/api/v1/git/callback"),
        ("GET", "/api/v1/git/branches?repo_url=https://github.com/a/b"),
    ] {
        let anonymous = http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"provider": "github", "token": GH_TOKEN, "base_url": base}).to_string(),
            ))
            .unwrap();
        assert_eq!(app.send(anonymous).await.status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn repositories_are_listed_page_by_page_in_the_providers_order() {
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let gl = connect(&app, "gitlab", &base, GL_TOKEN).await;
    let repos = |c: &Value| format!("/api/v1/git/connections/{}/repositories", c["id"].as_str().unwrap());

    // One page.
    let v = app.get(&repos(&gh)).await.json();
    assert_eq!(v["truncated"], false);
    let list = v["repositories"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(
        list[0],
        json!({
            "id": "1001", "full_name": "octocat/repo-0001", "name": "repo-0001", "owner": "octocat",
            "private": true, "archived": false, "default_branch": "main",
            "clone_url": format!("{base}/octocat/repo-0001.git"), "web_url": format!("{base}/octocat/repo-0001"),
            "description": "The first one", "updated_at": "2026-09-30T08:15:00Z",
        })
    );
    assert_eq!(
        (&list[1]["default_branch"], &list[1]["private"], &list[1]["description"]),
        (&json!("trunk"), &json!(false), &Value::Null)
    );
    assert_eq!(list[2]["archived"], true);

    let v = app.get(&repos(&gl)).await.json();
    let list = v["repositories"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(
        list[0],
        json!({
            "id": "501", "full_name": "acme/platform/app-0001", "name": "app-0001", "owner": "acme/platform",
            // `internal` isn't public either.
            "private": true, "archived": false, "default_branch": "main",
            "clone_url": format!("{base}/acme/platform/app-0001.git"), "web_url": format!("{base}/acme/platform/app-0001"),
            "description": null, "updated_at": "2026-09-29T10:00:00.123Z",
        })
    );
    // An empty project has no default branch; `public` is the only non-private visibility.
    assert_eq!((&list[1]["default_branch"], &list[1]["private"]), (&Value::Null, &json!(false)));
    assert_eq!(list[2]["private"], true);

    // Several pages, whether the provider says how many there are or not.
    fake.repositories.store(250, Ordering::SeqCst);
    for announce in [true, false] {
        fake.announce_pages.store(announce, Ordering::SeqCst);
        for (connection, first, last) in
            [(&gh, "octocat/repo-0001", "octocat/repo-0250"), (&gl, "acme/platform/app-0001", "acme/platform/app-0250")]
        {
            let v = app.get(&repos(connection)).await.json();
            let names: Vec<&str> =
                v["repositories"].as_array().unwrap().iter().map(|r| r["full_name"].as_str().unwrap()).collect();
            assert_eq!((names.len(), names[0], names[249]), (250, first, last), "announce={announce}");
            assert!(names.is_sorted(), "pages out of order (announce={announce})");
            assert_eq!(v["truncated"], false);
        }
    }

    // The listing stops at 1000 repositories, and says so.
    fake.repositories.store(1001, Ordering::SeqCst);
    for announce in [true, false] {
        fake.announce_pages.store(announce, Ordering::SeqCst);
        for connection in [&gh, &gl] {
            let before = fake.requests().len();
            let v = app.get(&repos(connection)).await.json();
            assert_eq!(v["repositories"].as_array().unwrap().len(), 1000, "announce={announce}");
            assert_eq!(v["truncated"], true, "announce={announce}");
            assert_eq!(fake.requests().len() - before, 10, "one request per page, 10 pages at most");
        }
    }
    fake.repositories.store(1000, Ordering::SeqCst);
    assert_eq!(app.get(&repos(&gh)).await.json()["truncated"], false);

    // Tokens travel as bearer tokens only: nothing in a URL.
    assert!(fake.requests().iter().all(|r| !r.contains("ghp_") && !r.contains("glpat-")), "{:?}", fake.requests());
    let r = app.get("/api/v1/git/connections/git-00000000000000000000/repositories").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_stored_token_the_provider_no_longer_accepts() {
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let id = gh["id"].as_str().unwrap();
    fake.revoke(GH_TOKEN);
    let r = app.get(&format!("/api/v1/git/connections/{id}/repositories")).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::CONFLICT, "git_authorization_rejected"), "{}", r.text());
    let message = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(message.contains("connect the GitHub account 'octocat' again with a new token"), "{message}");
    assert!(!message.contains("ghp_"), "{message}");
    // The connection is still there, to be given a new token.
    let again = connect(&app, "github", &base, GH_TOKEN_2).await;
    assert_eq!(again["id"], gh["id"]);
    let r = app.get(&format!("/api/v1/git/connections/{id}/repositories")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());

    // A provider that can't be reached is not the token's fault.
    let down = GitConnection::new(GitProvider::Gitlab, "http://127.0.0.1:9", "tanuki", GL_TOKEN);
    app.store.create_git_connection(&down).await.unwrap();
    let r = app.get(&format!("/api/v1/git/connections/{}/repositories", down.id)).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_GATEWAY, "git_provider_unavailable"), "{}", r.text());
    assert!(!r.text().contains("glpat-"), "{}", r.text());
}

#[tokio::test]
async fn github_is_authorized_by_registering_an_app_and_installing_it() {
    if app_key().is_none() {
        eprintln!("skipped: openssl is not installed");
        return;
    }
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    let authorize = |body: Value| app.post("/api/v1/git/authorize", body);
    let callback = |body: Value| app.post("/api/v1/git/callback", body);

    // 1. The browser is sent to GitHub with a manifest to post.
    let r = authorize(json!({"provider": "github", "base_url": base, "redirect_uri": REDIRECT})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let step = r.json();
    assert_eq!((&step["status"], &step["method"]), (&json!("redirect"), &json!("post")));
    let register = step["url"].as_str().unwrap().to_string();
    assert!(register.starts_with(&format!("{base}/settings/apps/new?state=")), "{register}");
    let manifest: Value = serde_json::from_str(step["fields"]["manifest"].as_str().unwrap()).unwrap();
    assert_eq!((&manifest["redirect_url"], &manifest["setup_url"]), (&json!(REDIRECT), &json!(REDIRECT)));
    assert_eq!(manifest["public"], false);
    assert_eq!(manifest["default_permissions"], json!({"contents": "read", "metadata": "read"}));
    // Nothing is stored, and GitHub wasn't asked anything yet.
    assert_eq!(app.get("/api/v1/git/connections").await.json(), json!([]));
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());

    // 2. GitHub registered the app and sends the browser back with a code:
    //    Ferry gets the app's secrets and sends the browser to its installation page.
    fake.registered("m4nifest-code", "octocat");
    let r = callback(json!({"state": state_of(&register), "code": "m4nifest-code"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let step = r.json();
    assert_eq!((&step["status"], &step["method"]), (&json!("redirect"), &json!("get")));
    let install = step["url"].as_str().unwrap().to_string();
    assert!(install.starts_with(&format!("{base}/apps/{APP_SLUG}/installations/new?state=")), "{install}");
    assert_ne!(state_of(&install), state_of(&register));
    assert_eq!(
        fake.requests(),
        vec![
            "POST /api/v3/app-manifests/m4nifest-code/conversions",
            "GET /api/v3/app/installations?per_page=100&page=1"
        ]
    );

    // The app waits for its installation as a pending connection, without any secret in sight.
    let listed = app.get("/api/v1/git/connections").await;
    let pending = listed.json()[0].clone();
    let id = pending["id"].as_str().unwrap().to_string();
    assert_eq!(
        (&pending["auth"], &pending["status"], &pending["account"]),
        (&json!("github_app"), &json!("pending"), &json!("octocat"))
    );
    assert_eq!(
        (&pending["app_slug"], &pending["app_url"]),
        (&json!(APP_SLUG), &json!(format!("{base}/apps/{APP_SLUG}")))
    );
    assert_eq!((&pending["token_hint"], &pending["manage_url"]), (&Value::Null, &Value::Null));
    assert_eq!(pending["client_id"], "Iv1.clientIdOfTheFakeApp");
    for secret in ["PRIVATE KEY", "s3cretOfTheFake", "private_key", "client_secret", "webhook_secret"] {
        assert!(!listed.text().contains(secret), "{secret}: {}", listed.text());
    }
    let stored = app.store.require_git_connection(&id).await.unwrap();
    assert_eq!(stored.auth, GitAuth::GithubApp);
    assert!(stored.app.as_ref().is_some_and(|a| a.id == APP_ID && a.private_key.contains("PRIVATE KEY")));
    // It can't list anything yet.
    let r = app.get(&format!("/api/v1/git/connections/{id}/repositories")).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::CONFLICT, "git_authorization_rejected"), "{}", r.text());
    assert!(r.text().contains("is not authorized yet: finish connecting it"), "{}", r.text());

    // What GitHub didn't send back is refused: a made-up state, a state
    // used twice, an installation of some other app.
    let r = callback(json!({"state": "made-up", "installation_id": 77})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, "invalid_request"), "{}", r.text());
    let r = callback(json!({"state": state_of(&register), "code": "m4nifest-code"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(r.text().contains("expired or was already used"), "{}", r.text());
    let r = callback(json!({"state": state_of(&install), "installation_id": 999, "setup_action": "install"})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, "git_authorization_rejected"), "{}", r.text());
    assert!(r.text().contains("999 is not an installation of the GitHub App 'ferry-test-app'"), "{}", r.text());

    // 3. The account installs the app (that state was spent: resume the connection).
    let r = authorize(json!({"connection_id": id, "redirect_uri": REDIRECT})).await;
    let install = r.json()["url"].as_str().unwrap().to_string();
    assert!(install.starts_with(&format!("{base}/apps/{APP_SLUG}/installations/new?state=")), "{}", r.text());
    fake.installed(77, "octocat", "selected");
    let r = callback(json!({"state": state_of(&install), "installation_id": 77, "setup_action": "install"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let done = r.json();
    assert_eq!(done["status"], "connected");
    let connection = &done["connection"];
    assert_eq!((&connection["id"], &connection["status"]), (&json!(id), &json!("connected")));
    assert_eq!(connection["repository_selection"], "selected");
    assert_eq!(connection["manage_url"], "https://github.example/settings/installations/77");
    assert_eq!((&connection["token_hint"], &connection["token_expires_at"]), (&Value::Null, &Value::Null));

    // The app's key mints an installation token, which lists what the
    // account allowed (most recently pushed first) and is kept for its hour.
    let repos = format!("/api/v1/git/connections/{id}/repositories");
    let r = app.get(&repos).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let names: Vec<String> =
        r.json()["repositories"].as_array().unwrap().iter().map(|r| r["full_name"].as_str().unwrap().into()).collect();
    assert_eq!(names, vec!["octocat/repo-0003", "octocat/repo-0002", "octocat/repo-0001"]);
    assert_eq!(app.get(&repos).await.status, StatusCode::OK);
    assert_eq!(fake.requests_to("/access_tokens"), 1, "{:?}", fake.requests());
    assert!(!fake.requests().iter().any(|r| r.contains("/user/repos")), "{:?}", fake.requests());
    // No token of any kind was stored.
    let stored = app.store.require_git_connection(&id).await.unwrap();
    assert_eq!((stored.token.as_str(), stored.refresh_token), ("", None));

    // Asking again when everything is in place connects at once.
    let r = authorize(json!({"connection_id": id, "redirect_uri": REDIRECT})).await;
    assert_eq!(r.json()["status"], "connected", "{}", r.text());

    // GitHub sends the browser back (without a state) after the account
    // changed the installation on GitHub's own pages: it is read again.
    fake.installed(77, "octocat", "all");
    let r = callback(json!({"state": "", "installation_id": 77, "setup_action": "update"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["connection"]["repository_selection"], "all");
    let r = callback(json!({"state": "", "installation_id": 5})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert_eq!(app.get(&repos).await.status, StatusCode::OK);

    // Uninstalled on GitHub: the token it had stops working, and no new one
    // can be minted. Both say to connect again; neither is a 401.
    fake.uninstalled(77);
    fake.tokens.lock().unwrap().retain(|token, _| !token.starts_with("ghs_"));
    for expected in ["connect the GitHub account 'octocat' again", "is no longer installed on 'octocat'"] {
        let r = app.get(&repos).await;
        assert_eq!((r.status, r.code().as_str()), (StatusCode::CONFLICT, "git_authorization_rejected"), "{}", r.text());
        assert!(r.text().contains(expected) && !r.text().contains("ghs_"), "{expected}: {}", r.text());
    }
    // Installing it again (GitHub hands out a new installation) repairs the connection.
    fake.installed(78, "octocat", "all");
    let r = authorize(json!({"connection_id": id, "redirect_uri": REDIRECT})).await;
    assert_eq!(r.json()["status"], "connected", "{}", r.text());
    assert_eq!(app.get(&repos).await.status, StatusCode::OK);

    // No secret ever travelled in a URL.
    assert!(fake.requests().iter().all(|r| !r.contains("ghs_") && !r.contains("eyJ")), "{:?}", fake.requests());
}

#[tokio::test]
async fn a_github_app_takes_over_the_account_it_is_registered_for() {
    if app_key().is_none() {
        eprintln!("skipped: openssl is not installed");
        return;
    }
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    // The account was connected with a token before.
    let pat = connect(&app, "github", &base, GH_TOKEN).await;
    let r = app
        .post("/api/v1/git/authorize", json!({"provider": "github", "base_url": base, "redirect_uri": REDIRECT}))
        .await;
    let register = r.json()["url"].as_str().unwrap().to_string();
    fake.registered("code-1", "OctoCat");
    // The app is already installed when GitHub sends the browser back (it
    // can be, when the account installs it right away): connected at once.
    fake.installed(12, "octocat", "all");
    let r = app.post("/api/v1/git/callback", json!({"state": state_of(&register), "code": "code-1"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let done = r.json();
    assert_eq!(done["status"], "connected", "{done}");
    // Same connection, now authorized through the app: the token is gone.
    assert_eq!((&done["connection"]["id"], &done["connection"]["auth"]), (&pat["id"], &json!("github_app")));
    assert_eq!((&done["connection"]["token_hint"], &done["connection"]["scopes"]), (&Value::Null, &json!([])));
    let stored = app.store.require_git_connection(pat["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(stored.token, "");
    assert_eq!(app.get("/api/v1/git/connections").await.json().as_array().unwrap().len(), 1);

    // In an organization: GitHub's page for the organization's apps.
    let r = app
        .post(
            "/api/v1/git/authorize",
            json!({"provider": "github", "base_url": base, "redirect_uri": REDIRECT, "organization": "acme"}),
        )
        .await;
    let url = r.json()["url"].as_str().unwrap().to_string();
    assert!(url.starts_with(&format!("{base}/organizations/acme/settings/apps/new?state=")), "{url}");
    // A registration GitHub doesn't know (the code expired) is said so.
    let r = app.post("/api/v1/git/callback", json!({"state": state_of(&url), "code": "stale"})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, "git_authorization_rejected"), "{}", r.text());
    assert!(r.text().contains("doesn't know this app registration"), "{}", r.text());

    // Bad requests never reach GitHub.
    let sent = fake.requests().len();
    for (body, word) in [
        (json!({"provider": "github"}), "redirect_uri"),
        (json!({"provider": "github", "redirect_uri": "javascript:alert(1)"}), "redirect_uri"),
        (json!({"provider": "github", "redirect_uri": format!("{REDIRECT}?x=1")}), "redirect_uri"),
        (json!({"redirect_uri": REDIRECT}), "provider is required"),
        (json!({"provider": "github", "redirect_uri": REDIRECT, "organization": "a/b"}), "organization"),
        (json!({"provider": "github", "redirect_uri": REDIRECT, "base_url": "ftp://x"}), "base_url"),
        (json!({"provider": "github", "redirect_uri": REDIRECT, "token": "x"}), "unknown field"),
        (json!({"connection_id": "git-00000000000000000000", "redirect_uri": REDIRECT}), "not found"),
    ] {
        let r = app.post("/api/v1/git/authorize", body.clone()).await;
        assert!(r.status == StatusCode::BAD_REQUEST || r.status == StatusCode::NOT_FOUND, "{body}: {}", r.text());
        assert!(r.text().contains(word), "{body}: {}", r.text());
    }
    assert_eq!(fake.requests().len(), sent);
}

#[tokio::test]
async fn gitlab_is_authorized_with_an_oauth_application() {
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    let authorize = |body: Value| app.post("/api/v1/git/authorize", body);
    let callback = |body: Value| app.post("/api/v1/git/callback", body);
    let rejected = "git_authorization_rejected";

    // GitLab can't register an application on the fly: it is asked for.
    let r = authorize(json!({"provider": "gitlab", "base_url": base, "redirect_uri": REDIRECT})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, "git_application_required"), "{}", r.text());
    assert!(r.text().contains("read_api, read_repository"), "{}", r.text());

    // With the application: GitLab's authorization page.
    let with_application = json!({
        "provider": "gitlab", "base_url": base, "redirect_uri": REDIRECT,
        "client_id": GL_APP_ID, "client_secret": GL_APP_KEY,
    });
    let r = authorize(with_application.clone()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let step = r.json();
    assert_eq!((&step["status"], &step["method"], &step["fields"]), (&json!("redirect"), &json!("get"), &json!({})));
    let page = step["url"].as_str().unwrap().to_string();
    let expected = format!(
        "{base}/oauth/authorize?client_id={GL_APP_ID}&redirect_uri=http%3A%2F%2Flocalhost%3A7878%2Fgit%2Fcallback\
         &response_type=code&state="
    );
    assert!(page.starts_with(&expected) && page.ends_with("&scope=read_api+read_repository"), "{page}");
    assert!(!r.text().contains(GL_APP_KEY));
    // The application waits as a pending connection; its key never comes back.
    let listed = app.get("/api/v1/git/connections").await;
    let pending = listed.json()[0].clone();
    assert_eq!(
        (&pending["auth"], &pending["status"], &pending["account"], &pending["client_id"]),
        (&json!("oauth"), &json!("pending"), &json!(""), &json!(GL_APP_ID))
    );
    assert!(!listed.text().contains(GL_APP_KEY), "{}", listed.text());
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());

    // The user refuses on GitLab's page.
    let r = callback(json!({
        "state": state_of(&page), "error": "access_denied",
        "error_description": "The resource owner or authorization server denied the request.",
    }))
    .await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, rejected), "{}", r.text());
    assert!(r.text().contains("GitLab did not authorize Ferry: The resource owner"), "{}", r.text());

    // Again, without typing the application again: it was kept.
    let r = authorize(json!({"provider": "gitlab", "base_url": base, "redirect_uri": REDIRECT})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let page = r.json()["url"].as_str().unwrap().to_string();
    // A code GitLab doesn't know (or one for another redirect URI).
    let r = callback(json!({"state": state_of(&page), "code": "not-a-code"})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, rejected), "{}", r.text());
    assert!(
        r.text().contains("GitLab refused the authorization: The provided authorization grant is invalid"),
        "{}",
        r.text()
    );
    assert!(!r.text().contains(GL_APP_KEY) && !r.text().contains("not-a-code"), "{}", r.text());

    // The account authorizes: the code is exchanged for tokens, and the
    // account read with them.
    let r = authorize(json!({"provider": "gitlab", "base_url": base, "redirect_uri": REDIRECT})).await;
    let page = r.json()["url"].as_str().unwrap().to_string();
    fake.authorized("c0de-1", "tanuki");
    let r = callback(json!({"state": state_of(&page), "code": "c0de-1"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let done = r.json();
    assert_eq!(done["status"], "connected");
    let connection = done["connection"].clone();
    let id = connection["id"].as_str().unwrap().to_string();
    assert_eq!(id, pending["id"].as_str().unwrap(), "the pending connection became the account's");
    assert_eq!(
        (&connection["auth"], &connection["status"], &connection["account"], &connection["account_name"]),
        (&json!("oauth"), &json!("connected"), &json!("tanuki"), &json!("Tanuki"))
    );
    assert_eq!(connection["scopes"], json!(["read_api", "read_repository"]));
    // Its tokens are renewed: no hint, no expiry to watch.
    assert_eq!((&connection["token_hint"], &connection["token_expires_at"]), (&Value::Null, &Value::Null));
    assert!(!r.text().contains("gl-access-") && !r.text().contains("gl-refresh-"), "{}", r.text());
    let stored = app.store.require_git_connection(&id).await.unwrap();
    assert_eq!(
        (stored.token.as_str(), stored.refresh_token.as_deref()),
        ("gl-access-1-tanuki", Some("gl-refresh-1-tanuki"))
    );
    let lifetime = stored.token_expires_at.unwrap() - Utc::now();
    assert!(lifetime > chrono::Duration::minutes(110) && lifetime <= chrono::Duration::hours(2), "{lifetime}");
    // An OAuth token can't describe itself the way a personal one can: not asked.
    assert_eq!(fake.requests_to("/personal_access_tokens/self"), 0, "{:?}", fake.requests());
    assert_eq!(app.get("/api/v1/git/connections").await.json().as_array().unwrap().len(), 1);

    // It lists the account's projects.
    let repos = format!("/api/v1/git/connections/{id}/repositories");
    let r = app.get(&repos).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["repositories"][0]["full_name"], "acme/platform/app-0001");
    assert_eq!(fake.requests_to("/oauth/token"), 2, "one refused exchange and one that worked");

    // About to expire: renewed first (the old access token stops working,
    // the refresh token is replaced), without the connection "changing".
    let soon = Utc::now() + chrono::Duration::minutes(2);
    app.store.set_git_tokens(&id, "gl-access-1-tanuki", Some("gl-refresh-1-tanuki"), Some(soon)).await.unwrap();
    let r = app.get(&repos).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let renewed = app.store.require_git_connection(&id).await.unwrap();
    assert_eq!(
        (renewed.token.as_str(), renewed.refresh_token.as_deref()),
        ("gl-access-2-tanuki", Some("gl-refresh-2-tanuki"))
    );
    assert_eq!(renewed.updated_at, stored.updated_at);
    assert_eq!(fake.requests_to("/oauth/token"), 3);
    // Still good for two hours: no renewal.
    assert_eq!(app.get(&repos).await.status, StatusCode::OK);
    assert_eq!(fake.requests_to("/oauth/token"), 3);

    // The authorization was revoked on GitLab: the renewal is refused, and says what to do.
    fake.refresh_tokens.lock().unwrap().clear();
    app.store.set_git_tokens(&id, "gl-access-2-tanuki", Some("gl-refresh-2-tanuki"), Some(soon)).await.unwrap();
    let r = app.get(&repos).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::CONFLICT, rejected), "{}", r.text());
    assert!(r.text().contains("authorize the GitLab account 'tanuki' again"), "{}", r.text());
    assert!(!r.text().contains("gl-refresh-") && !r.text().contains(GL_APP_KEY), "{}", r.text());

    // Authorizing the connection again repairs it, in place.
    let r = authorize(json!({"connection_id": id, "redirect_uri": REDIRECT})).await;
    let page = r.json()["url"].as_str().unwrap().to_string();
    assert!(page.contains(&format!("client_id={GL_APP_ID}&")), "{page}");
    fake.authorized("c0de-2", "tanuki");
    let r = callback(json!({"state": state_of(&page), "code": "c0de-2"})).await;
    assert_eq!((r.status, &r.json()["connection"]["id"]), (StatusCode::OK, &json!(id)), "{}", r.text());
    assert_eq!(app.get(&repos).await.status, StatusCode::OK);

    // Another account authorizes the same application: its own connection.
    let r = authorize(json!({"provider": "gitlab", "base_url": base, "redirect_uri": REDIRECT})).await;
    fake.authorized("c0de-3", "otter");
    let page = r.json()["url"].as_str().unwrap().to_string();
    let r = callback(json!({"state": state_of(&page), "code": "c0de-3"})).await;
    assert_eq!(r.json()["connection"]["account"], "otter", "{}", r.text());
    assert_ne!(r.json()["connection"]["id"], id);
    assert_eq!(app.get("/api/v1/git/connections").await.json().as_array().unwrap().len(), 2);

    // A wrong application key: GitLab says so when the code is exchanged.
    let wrong = json!({
        "provider": "gitlab", "base_url": format!("{base}/"), "redirect_uri": REDIRECT,
        "client_id": GL_APP_ID, "client_secret": GL_OTHER_KEY,
    });
    let page = authorize(wrong).await.json()["url"].as_str().unwrap().to_string();
    fake.authorized("c0de-4", "tanuki");
    let r = callback(json!({"state": state_of(&page), "code": "c0de-4"})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, rejected), "{}", r.text());
    assert!(r.text().contains("GitLab refused the authorization: Client authentication failed"), "{}", r.text());
    assert!(!r.text().contains("gloas-"), "{}", r.text());
    // Half an application isn't one.
    let r = authorize(json!({"provider": "gitlab", "redirect_uri": REDIRECT, "client_id": GL_APP_ID})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(r.text().contains("client_id and client_secret go together"), "{}", r.text());
}

#[tokio::test]
async fn branches_are_read_from_the_repository_itself() {
    if !have("git") {
        eprintln!("skipped: git is not installed");
        return;
    }
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    let dir = tempfile::tempdir().unwrap();
    let (work, bare) = sample_repository(dir.path());
    *fake.repo.lock().unwrap() = Some(bare);
    let branches = async |repo_url: &str| {
        let query: String = form_urlencoded::Serializer::new(String::new()).append_pair("repo_url", repo_url).finish();
        app.get(&format!("/api/v1/git/branches?{query}")).await
    };

    // Any repository a service can deploy from — here a path on the server:
    // the default branch first, then by name.
    let r = branches(&work.display().to_string()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"default_branch": "main", "branches": ["main", "dev", "feature/login"], "connection_id": null})
    );

    // A private repository on the provider: refused without an account...
    let private = format!("{base}/octocat/repo-0001.git");
    let r = branches(&private).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_GATEWAY, "git_remote_unreachable"), "{}", r.text());
    let message = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(message.starts_with(&format!("cannot list the branches of {private}: ")), "{message}");
    assert!(message.ends_with("if the repository is private, connect its GitHub or GitLab account to this server: it is then cloned with that account"), "{message}");

    // ...and read with the server's connection for that host, nobody
    // naming it: connections belong to the server.
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let r = branches(&private).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let v = r.json();
    assert_eq!(
        (&v["default_branch"], &v["branches"]),
        (&json!("main"), &json!(["main", "dev", "feature/login"])),
        "{v}"
    );
    assert_eq!(v["connection_id"], gh["id"]);
    assert_eq!(
        fake.git_logins.lock().unwrap().first().map(String::as_str),
        Some(&*format!("x-access-token:{GH_TOKEN}"))
    );
    // The token went to git through its environment: never in a URL.
    assert!(fake.requests().iter().all(|r| !r.contains("ghp_")), "{:?}", fake.requests());

    // A GitLab account reads its own host's repositories as `oauth2`.
    app.delete(&format!("/api/v1/git/connections/{}?force=true", gh["id"].as_str().unwrap())).await;
    let gl = connect(&app, "gitlab", &base, GL_TOKEN).await;
    fake.git_logins.lock().unwrap().clear();
    let r = branches(&format!("{base}/acme/platform/app-0001.git")).await;
    assert_eq!((r.status, &r.json()["connection_id"]), (StatusCode::OK, &gl["id"]), "{}", r.text());
    assert_eq!(fake.git_logins.lock().unwrap().first().map(String::as_str), Some(&*format!("oauth2:{GL_TOKEN}")));

    // A token the remote refuses: what to do about it.
    fake.revoke(GL_TOKEN);
    let r = branches(&private).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_GATEWAY, "git_remote_unreachable"), "{}", r.text());
    assert!(r.text().contains("the token of the GitLab account 'tanuki' may have expired"), "{}", r.text());
    assert!(!r.text().contains("glpat-"), "{}", r.text());

    // What isn't a repository, or a repository URL.
    let r = branches(&dir.path().join("nothing-here").display().to_string()).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_GATEWAY, "git_remote_unreachable"), "{}", r.text());
    for bad in ["", "   ", "ext::sh -c id", "relative/path", "ftp://example.com/a.git"] {
        let r = branches(bad).await;
        assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, "invalid_request"), "{bad}: {}", r.text());
    }
    assert_eq!(app.get("/api/v1/git/branches").await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn services_are_cloned_with_the_connection_that_serves_their_repository() {
    let app = TestApp::new().await;
    let (_fake, base) = fake_provider().await;
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let id = gh["id"].as_str().unwrap();
    let repo = format!("{base}/octocat/repo-0001.git");
    let services = async || app.get(&format!("/api/v1/git/connections/{id}")).await.json()["services"].clone();

    // A service names its repository, never a connection.
    let v = app.create_service(json!({"name": "app", "repo_url": repo})).await;
    assert!(v.get("git_connection_id").is_none(), "{v}");
    assert_eq!(
        v["latest_deploy"]["source"],
        json!({"kind": "git", "repo_url": repo, "branch": "main", "commit": null})
    );
    let r = app.post("/api/v1/services", json!({"name": "x", "repo_url": repo, "git_connection_id": id})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(r.text().contains("unknown field"), "{}", r.text());
    let r = app.patch("/api/v1/services/app", json!({"git_connection_id": id})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());

    // The connection says whose repositories it clones: those on its host.
    assert_eq!(services().await, json!(["app"]));
    app.create_service(json!({"name": "elsewhere", "repo_url": "https://github.com/a/b"})).await;
    app.create_service(json!({"name": "ssh", "repo_url": "git@127.0.0.1:octocat/repo-0001.git"})).await;
    app.create_service(json!({"name": "own-credentials", "repo_url": repo.replace("http://", "http://u:p@")})).await;
    app.create_service(json!({"name": "image", "image": "nginx:alpine"})).await;
    app.create_service(json!({"name": "a-second", "repo_url": format!("{base}/acme/api.git")})).await;
    assert_eq!(services().await, json!(["a-second", "app"]));
    // Moving a service's repository moves it in or out.
    app.patch("/api/v1/services/elsewhere", json!({"repo_url": format!("{base}/octocat/repo-0002.git")})).await;
    assert_eq!(services().await, json!(["a-second", "app", "elsewhere"]));
    app.patch("/api/v1/services/app", json!({"repo_url": "https://github.com/octocat/app.git"})).await;
    assert_eq!(services().await, json!(["a-second", "elsewhere"]));

    // Disconnecting an account that clones for services needs force; they keep their repository.
    let r = app.delete(&format!("/api/v1/git/connections/{id}")).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::CONFLICT, "conflict"), "{}", r.text());
    assert!(
        r.text().contains("the GitHub account 'octocat' clones the repository of a-second, elsewhere"),
        "{}",
        r.text()
    );
    assert_eq!(app.delete(&format!("/api/v1/git/connections/{id}?force=true")).await.status, StatusCode::NO_CONTENT);
    let v = app.get("/api/v1/services/elsewhere").await.json();
    assert_eq!(v["repo_url"], format!("{base}/octocat/repo-0002.git"));
    assert_eq!(app.get("/api/v1/git/connections").await.json(), json!([]));
    assert_eq!(app.delete(&format!("/api/v1/git/connections/{id}")).await.status, StatusCode::NOT_FOUND);

    // One that clones for nobody goes without force.
    let idle = GitConnection::new(GitProvider::Gitlab, "https://gitlab.example.com", "tanuki", GL_TOKEN);
    app.store.create_git_connection(&idle).await.unwrap();
    assert_eq!(app.delete(&format!("/api/v1/git/connections/{}", idle.id)).await.status, StatusCode::NO_CONTENT);
}
