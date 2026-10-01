//! Git connections: connecting GitHub / GitLab accounts with an access
//! token, listing their repositories and branches, and services cloned
//! through them — against a fake provider on a local socket.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use common::TestApp;
use ferry_core::{GitConnection, GitProvider};
use http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

const GH_TOKEN: &str = "ghp_validTokenOfOctocat0001";
const GH_TOKEN_2: &str = "ghp_renewedTokenOfOctocat9XYZ";
const GL_TOKEN: &str = "glpat-validTokenOfTanuki-7Qr";
const LIMITED: &str = "ghp_rateLimitedToken";
const NO_SCOPE: &str = "glpat-withoutTheApiScope";

/// A fake GitHub Enterprise Server (`/api/v3`) and GitLab (`/api/v4`) on one
/// address. Every request needs `Authorization: Bearer <known token>`.
struct Fake {
    /// token → login.
    tokens: Mutex<HashMap<String, String>>,
    /// How many repositories / projects every account sees.
    repositories: AtomicUsize,
    /// Listings say how many pages they have (GitHub: `rel="last"`, GitLab: `x-total-pages`).
    announce_pages: AtomicBool,
    /// `METHOD path?query` of every request, in order.
    requests: Mutex<Vec<String>>,
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

async fn gh_repos(State(fake): FakeState, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(login) = fake.login(&headers) else { return github_denied() };
    assert_eq!(q.get("sort").map(String::as_str), Some("pushed"), "{q:?}");
    let (base, page) = (own_url(&headers), page_of(&q));
    let (items, pages) = paginate(fake.repositories.load(Ordering::SeqCst), page);
    let repos: Vec<Value> = items
        .map(|i| {
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
        })
        .collect();
    let mut link = Vec::new();
    if page < pages {
        link.push(format!("<{base}/api/v3/user/repos?per_page=100&page={}>; rel=\"next\"", page + 1));
        if fake.announce_pages.load(Ordering::SeqCst) {
            link.push(format!("<{base}/api/v3/user/repos?per_page=100&page={pages}>; rel=\"last\""));
        }
    }
    ([("link", link.join(", "))], axum::Json(repos)).into_response()
}

async fn gh_branches(
    State(fake): FakeState,
    headers: HeaderMap,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if fake.login(&headers).is_none() {
        return github_denied();
    }
    let base = own_url(&headers);
    match (owner.as_str(), repo.as_str()) {
        ("octocat", "repo-0001") => axum::Json(json!([
            {"name": "main", "protected": true, "commit": {"sha": "a".repeat(40)}},
            {"name": "feature/login", "protected": false, "commit": {"sha": "b".repeat(40)}},
        ]))
        .into_response(),
        ("octocat", "many-branches") => {
            let page = page_of(&q);
            let (items, pages) = paginate(230, page);
            let branches: Vec<Value> = items.map(|i| json!({"name": format!("b{i:03}"), "protected": false})).collect();
            let link = if page < pages {
                let at =
                    |n: usize| format!("<{base}/api/v3/repos/octocat/many-branches/branches?per_page=100&page={n}>");
                format!("{}; rel=\"next\", {}; rel=\"last\"", at(page + 1), at(pages))
            } else {
                String::new()
            };
            ([("link", link)], axum::Json(branches)).into_response()
        }
        _ => (StatusCode::NOT_FOUND, axum::Json(json!({"message": "Not Found"}))).into_response(),
    }
}

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

async fn gl_branches(State(fake): FakeState, headers: HeaderMap, Path(project): Path<String>) -> Response {
    if fake.login(&headers).is_none() {
        return gitlab_denied();
    }
    // The whole path arrives as one escaped segment.
    if project == "acme/platform/app-0001" {
        axum::Json(json!([
            {"name": "main", "protected": true, "default": true, "merged": false},
            {"name": "release/1.x", "protected": false, "default": false, "merged": false},
        ]))
        .into_response()
    } else {
        (StatusCode::NOT_FOUND, axum::Json(json!({"message": "404 Project Not Found"}))).into_response()
    }
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
        requests: Mutex::new(Vec::new()),
    });
    let router = Router::new()
        .route("/api/v3/user", get(gh_user))
        .route("/api/v3/user/repos", get(gh_repos))
        .route("/api/v3/repos/{owner}/{repo}/branches", get(gh_branches))
        .route("/api/v4/user", get(gl_user))
        .route("/api/v4/personal_access_tokens/self", get(gl_token))
        .route("/api/v4/projects", get(gl_projects))
        .route("/api/v4/projects/{project}/repository/branches", get(gl_branches))
        .layer(axum::middleware::from_fn_with_state(fake.clone(), record))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (fake, base)
}

/// Connect an account through the API; returns its view.
async fn connect(app: &TestApp, provider: &str, base: &str, token: &str) -> Value {
    let r = app.post("/api/v1/git/connections", json!({"provider": provider, "token": token, "base_url": base})).await;
    assert!(r.status.is_success(), "{} {}", r.status, r.text());
    r.json()
}

#[tokio::test]
async fn connecting_asks_the_provider_and_never_returns_the_token() {
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    assert_eq!(app.get("/api/v1/git/connections").await.json(), json!([]));

    let r =
        app.post("/api/v1/git/connections", json!({"provider": "github", "token": GH_TOKEN, "base_url": base})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let v = r.json();
    let id = v["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("git-") && id.len() == 24, "{id}");
    assert_eq!(v["provider"], "github");
    assert_eq!(v["base_url"], base);
    assert_eq!((&v["account"], &v["account_name"]), (&json!("octocat"), &json!("The Octocat")));
    assert_eq!(v["scopes"], json!(["repo", "read:org"]));
    assert_eq!(v["token_expires_at"], "2027-01-01T00:00:00Z");
    assert_eq!(v["services"], json!([]));
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

    // Never a 401: that would mean "wrong Ferry token" to API clients.
    let r = post(json!({"provider": "github", "token": "ghp_notAValidToken", "base_url": base})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, "git_token_rejected"), "{}", r.text());
    let message = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(message.starts_with("GitHub rejected the token: Bad credentials"), "{message}");
    assert!(!message.contains("ghp_"), "{message}");

    let r = post(json!({"provider": "gitlab", "token": NO_SCOPE, "base_url": base})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::BAD_REQUEST, "git_token_rejected"), "{}", r.text());
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

    // Like the rest of the API, it needs the Ferry token.
    let anonymous = http::Request::post("/api/v1/git/connections")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(json!({"provider": "github", "token": GH_TOKEN, "base_url": base}).to_string()))
        .unwrap();
    assert_eq!(app.send(anonymous).await.status, StatusCode::UNAUTHORIZED);
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
async fn branches_of_a_repository() {
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let gl = connect(&app, "gitlab", &base, GL_TOKEN).await;
    let branches = async |c: &Value, repository: &str| {
        let id = c["id"].as_str().unwrap();
        app.get(&format!("/api/v1/git/connections/{id}/branches?repository={repository}")).await
    };

    let r = branches(&gh, "octocat/repo-0001").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!([{"name": "main", "protected": true}, {"name": "feature/login", "protected": false}]));
    // GitLab takes the whole path (subgroups included) as one escaped segment.
    let r = branches(&gl, "acme%2Fplatform%2Fapp-0001").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!([{"name": "main", "protected": true}, {"name": "release/1.x", "protected": false}]));
    assert_eq!(
        fake.requests_to("/api/v4/projects/acme%2Fplatform%2Fapp-0001/repository/branches"),
        1,
        "{:?}",
        fake.requests()
    );

    // Every page of a repository with many branches.
    let many = branches(&gh, "octocat/many-branches").await.json();
    let names: Vec<&str> = many.as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!((names.len(), names[0], names[229]), (230, "b001", "b230"));

    // Unknown repositories, and names that aren't repositories.
    for (connection, repository) in [(&gh, "octocat/nope"), (&gl, "acme/nope")] {
        let r = branches(connection, repository).await;
        assert_eq!((r.status, r.code().as_str()), (StatusCode::NOT_FOUND, "not_found"), "{}", r.text());
        assert!(r.text().contains(&format!("repository '{repository}' not found on")), "{}", r.text());
    }
    let sent = fake.requests().len();
    for (connection, repository) in [
        (&gh, "repo-0001"),
        (&gh, "octocat/repo-0001/extra"),
        (&gh, "octocat/..%2F..%2Fuser"),
        (&gl, "acme/../../user"),
        (&gl, "acme/app%3Fx"),
        (&gl, ""),
    ] {
        let r = branches(connection, repository).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{repository}: {}", r.text());
    }
    let id = gh["id"].as_str().unwrap();
    assert_eq!(app.get(&format!("/api/v1/git/connections/{id}/branches")).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(fake.requests().len(), sent, "malformed names never reach the provider");
}

#[tokio::test]
async fn a_stored_token_the_provider_no_longer_accepts() {
    let app = TestApp::new().await;
    let (fake, base) = fake_provider().await;
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let id = gh["id"].as_str().unwrap();
    fake.revoke(GH_TOKEN);
    for path in ["repositories", "branches?repository=octocat/repo-0001"] {
        let r = app.get(&format!("/api/v1/git/connections/{id}/{path}")).await;
        assert_eq!((r.status, r.code().as_str()), (StatusCode::CONFLICT, "git_token_rejected"), "{}", r.text());
        let message = r.json()["error"]["message"].as_str().unwrap().to_string();
        assert!(message.contains("connect the GitHub account 'octocat' again with a new token"), "{message}");
        assert!(!message.contains("ghp_"), "{message}");
    }
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
async fn services_clone_through_a_connection_of_their_repositorys_host() {
    let app = TestApp::new().await;
    let (_fake, base) = fake_provider().await;
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let id = gh["id"].as_str().unwrap();
    let repo = format!("{base}/octocat/repo-0001.git");

    let v = app.create_service(json!({"name": "app", "repo_url": repo, "git_connection_id": id})).await;
    assert_eq!((&v["git_connection_id"], &v["repo_url"]), (&json!(id), &json!(repo)));
    assert_eq!(
        v["latest_deploy"]["source"],
        json!({"kind": "git", "repo_url": repo, "branch": "main", "commit": null})
    );
    assert_eq!(app.get(&format!("/api/v1/git/connections/{id}")).await.json()["services"], json!(["app"]));
    // Services without one say so.
    let plain = app.create_service(json!({"name": "public", "repo_url": "https://github.com/a/b"})).await;
    assert_eq!(plain["git_connection_id"], Value::Null);

    // A connection's token is for http(s) repositories of its own host only.
    for (body, word) in [
        (json!({"name": "x", "repo_url": repo, "git_connection_id": "git-00000000000000000000"}), "not found"),
        (
            json!({"name": "x", "repo_url": "https://github.com/octocat/app.git", "git_connection_id": id}),
            "can't be used for",
        ),
        (
            json!({"name": "x", "repo_url": "git@127.0.0.1:octocat/app.git", "git_connection_id": id}),
            "can't be used for",
        ),
        (
            json!({"name": "x", "repo_url": repo.replace("http://", "http://u:s3cret@"), "git_connection_id": id}),
            "can't be used for",
        ),
        (json!({"name": "x", "repo_url": "/srv/repos/app", "git_connection_id": id}), "can't be used for"),
    ] {
        let r = app.post("/api/v1/services", body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
        assert!(r.text().contains(word) && !r.text().contains("s3cret"), "{body}: {}", r.text());
    }
    // Without a repository there is nothing to clone: the connection is dropped.
    let image = app.create_service(json!({"name": "img", "image": "nginx:alpine", "git_connection_id": id})).await;
    assert_eq!(image["git_connection_id"], Value::Null);

    // PATCH: the connection follows the repository on its host...
    let other = format!("{base}/octocat/repo-0002.git");
    let v = app.patch("/api/v1/services/app", json!({"repo_url": other, "branch": "trunk"})).await.json();
    assert_eq!((&v["git_connection_id"], &v["repo_url"]), (&json!(id), &json!(other)));
    // ...is dropped when the repository moves elsewhere...
    let v = app.patch("/api/v1/services/app", json!({"repo_url": "https://github.com/octocat/app.git"})).await.json();
    assert_eq!(v["git_connection_id"], Value::Null);
    // ...can't be set for a repository of another host...
    let r = app.patch("/api/v1/services/app", json!({"git_connection_id": id})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert!(r.text().contains("can't be used for https://github.com/octocat/app.git"), "{}", r.text());
    // ...and is set, replaced or removed explicitly.
    let v = app.patch("/api/v1/services/app", json!({"repo_url": repo, "git_connection_id": id})).await.json();
    assert_eq!(v["git_connection_id"], id);
    let v = app.patch("/api/v1/services/app", json!({"git_connection_id": ""})).await.json();
    assert_eq!((&v["git_connection_id"], &v["repo_url"]), (&Value::Null, &json!(repo)));
    let v = app.patch("/api/v1/services/app", json!({"git_connection_id": id})).await.json();
    assert_eq!(v["git_connection_id"], id);
    let r = app.patch("/api/v1/services/app", json!({"git_connection_id": "git-00000000000000000000"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    // Unrelated changes keep it.
    let v = app.patch("/api/v1/services/app", json!({"start_command": "node server.js"})).await.json();
    assert_eq!(v["git_connection_id"], id);
    // Switching to an image forgets the repository and its connection.
    let v = app.patch("/api/v1/services/public", json!({"repo_url": repo, "git_connection_id": id})).await.json();
    assert_eq!(v["git_connection_id"], id);
    let v = app.patch("/api/v1/services/public", json!({"image": "nginx:alpine", "repo_url": ""})).await.json();
    assert_eq!((&v["git_connection_id"], &v["repo_url"]), (&Value::Null, &Value::Null));

    // Disconnecting an account services use needs force; they keep their repository.
    let r = app.delete(&format!("/api/v1/git/connections/{id}")).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::CONFLICT, "conflict"), "{}", r.text());
    assert!(r.text().contains("the GitHub account 'octocat' is used by app"), "{}", r.text());
    assert_eq!(app.delete(&format!("/api/v1/git/connections/{id}?force=true")).await.status, StatusCode::NO_CONTENT);
    let v = app.get("/api/v1/services/app").await.json();
    assert_eq!((&v["git_connection_id"], &v["repo_url"]), (&Value::Null, &json!(repo)));
    assert_eq!(app.get("/api/v1/git/connections").await.json(), json!([]));
    assert_eq!(app.delete(&format!("/api/v1/git/connections/{id}")).await.status, StatusCode::NOT_FOUND);

    // One nobody uses goes without force.
    let gl = connect(&app, "gitlab", &base, GL_TOKEN).await;
    let r = app.delete(&format!("/api/v1/git/connections/{}", gl["id"].as_str().unwrap())).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn blueprints_keep_a_connection_while_the_repository_stays_on_its_host() {
    let app = TestApp::new().await;
    let (_fake, base) = fake_provider().await;
    let gh = connect(&app, "github", &base, GH_TOKEN).await;
    let id = gh["id"].as_str().unwrap();
    let repo = |n: u32| format!("{base}/octocat/repo-000{n}.git");
    app.create_service(json!({"name": "app", "repo_url": repo(1), "git_connection_id": id})).await;
    let apply = |repo: String| {
        let yaml = format!("services:\n  - type: web\n    name: app\n    repo: {repo}\n");
        app.post("/api/v1/blueprints/apply", json!({"yaml": yaml}))
    };

    // Unchanged, or another repository of the same host: kept.
    for n in [1, 2] {
        let r = apply(repo(n)).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        let v = app.get("/api/v1/services/app").await.json();
        assert_eq!((&v["repo_url"], &v["git_connection_id"]), (&json!(repo(n)), &json!(id)));
    }
    // Another host: the token isn't for it.
    let r = apply("https://github.com/octocat/app.git".to_string()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let changes = r.json()["actions"][0]["changes"].to_string();
    assert!(changes.contains(&format!("git_connection_id: {id} → (none)")), "{changes}");
    let v = app.get("/api/v1/services/app").await.json();
    assert_eq!(v["git_connection_id"], Value::Null);
}
