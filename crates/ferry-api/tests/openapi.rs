//! The OpenAPI document (`/api/openapi.json`) and Swagger UI (`/api/docs`).
//!
//! [`ROUTES`] is the authoritative list of the router's operations: every
//! entry must be documented, every documented operation must be listed, every
//! entry must reach a handler of the router, and every `.route(...)` of
//! `src/lib.rs` must be listed.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use axum::body::Body;
use common::{TOKEN, TestApp};
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// Every operation of the router, as documented (method, OpenAPI path).
const ROUTES: &[(&str, &str)] = &[
    ("GET", "/healthz"),
    ("GET", "/api/openapi.json"),
    ("GET", "/api/v1/info"),
    ("GET", "/api/v1/events"),
    // the account, its sessions and API tokens, `ferry login`
    ("GET", "/api/v1/auth/status"),
    ("POST", "/api/v1/auth/setup"),
    ("POST", "/api/v1/auth/login"),
    ("POST", "/api/v1/auth/logout"),
    ("POST", "/api/v1/auth/password"),
    ("GET", "/api/v1/auth/sessions"),
    ("DELETE", "/api/v1/auth/sessions/{id}"),
    ("GET", "/api/v1/auth/tokens"),
    ("POST", "/api/v1/auth/tokens"),
    ("DELETE", "/api/v1/auth/tokens/{id}"),
    ("POST", "/api/v1/auth/cli"),
    ("GET", "/api/v1/auth/cli/{id}"),
    ("POST", "/api/v1/auth/cli/{id}/approve"),
    ("POST", "/api/v1/auth/cli/{id}/deny"),
    ("POST", "/api/v1/auth/cli/{id}/token"),
    // services
    ("GET", "/api/v1/services"),
    ("POST", "/api/v1/services"),
    ("GET", "/api/v1/services/{id}"),
    ("PATCH", "/api/v1/services/{id}"),
    ("DELETE", "/api/v1/services/{id}"),
    ("GET", "/api/v1/services/{id}/status"),
    ("GET", "/api/v1/services/{id}/logs"),
    ("POST", "/api/v1/services/{id}/restart"),
    ("POST", "/api/v1/services/{id}/suspend"),
    ("POST", "/api/v1/services/{id}/resume"),
    ("POST", "/api/v1/services/{id}/scale"),
    ("POST", "/api/v1/services/{id}/rollback"),
    ("POST", "/api/v1/services/{id}/deploy-hook/rotate"),
    // deploys
    ("GET", "/api/v1/services/{id}/deploys"),
    ("POST", "/api/v1/services/{id}/deploys"),
    ("POST", "/api/v1/services/{id}/deploys/upload"),
    ("GET", "/api/v1/deploys/{deploy_id}"),
    ("POST", "/api/v1/deploys/{deploy_id}/cancel"),
    ("GET", "/api/v1/deploys/{deploy_id}/logs"),
    // env vars & env group links
    ("GET", "/api/v1/services/{id}/env"),
    ("PUT", "/api/v1/services/{id}/env"),
    ("PATCH", "/api/v1/services/{id}/env"),
    ("POST", "/api/v1/services/{id}/env-groups"),
    ("DELETE", "/api/v1/services/{id}/env-groups/{group}"),
    // custom domains
    ("GET", "/api/v1/services/{id}/domains"),
    ("POST", "/api/v1/services/{id}/domains"),
    ("DELETE", "/api/v1/services/{id}/domains/{domain}"),
    // the domains services are served under, and certificates
    ("GET", "/api/v1/domains"),
    ("POST", "/api/v1/domains"),
    ("GET", "/api/v1/domains/{id}"),
    ("PATCH", "/api/v1/domains/{id}"),
    ("DELETE", "/api/v1/domains/{id}"),
    ("POST", "/api/v1/domains/{id}/verify"),
    ("GET", "/api/v1/certificates"),
    // jobs
    ("GET", "/api/v1/services/{id}/jobs"),
    ("POST", "/api/v1/services/{id}/jobs"),
    ("GET", "/api/v1/jobs/{job_id}"),
    ("POST", "/api/v1/jobs/{job_id}/cancel"),
    ("GET", "/api/v1/jobs/{job_id}/logs"),
    // datastores
    ("GET", "/api/v1/datastores"),
    ("POST", "/api/v1/datastores"),
    ("GET", "/api/v1/datastores/{id}"),
    ("PATCH", "/api/v1/datastores/{id}"),
    ("DELETE", "/api/v1/datastores/{id}"),
    // env groups
    ("GET", "/api/v1/env-groups"),
    ("POST", "/api/v1/env-groups"),
    ("GET", "/api/v1/env-groups/{id}"),
    ("DELETE", "/api/v1/env-groups/{id}"),
    ("PUT", "/api/v1/env-groups/{id}/env"),
    ("PATCH", "/api/v1/env-groups/{id}/env"),
    // git connections
    ("GET", "/api/v1/git/connections"),
    ("POST", "/api/v1/git/connections"),
    ("GET", "/api/v1/git/connections/{id}"),
    ("DELETE", "/api/v1/git/connections/{id}"),
    ("GET", "/api/v1/git/connections/{id}/repositories"),
    ("POST", "/api/v1/git/authorize"),
    ("POST", "/api/v1/git/callback"),
    ("GET", "/api/v1/git/branches"),
    // blueprints
    ("POST", "/api/v1/blueprints/apply"),
    // webhooks
    ("GET", "/hooks/deploy/{service_id}"),
    ("POST", "/hooks/deploy/{service_id}"),
    ("POST", "/hooks/github"),
];

/// Documented routes that aren't `.route(...)` lines of `src/lib.rs`.
const NOT_IN_LIB_RS: &[(&str, &str)] = &[("GET", "/api/openapi.json")];

/// The `/api/v1` operations that need no authentication.
const PUBLIC: &[&str] = &[
    "/api/v1/auth/status",
    "/api/v1/auth/setup",
    "/api/v1/auth/login",
    "/api/v1/auth/logout",
    "/api/v1/auth/cli",
    "/api/v1/auth/cli/{id}/token",
];

const TAGS: &[&str] = &[
    "info",
    "auth",
    "services",
    "deploys",
    "env",
    "env-groups",
    "domains",
    "jobs",
    "datastores",
    "git",
    "blueprints",
    "events",
    "hooks",
];

const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace"];

async fn document(app: &TestApp) -> Value {
    // No token: the document is public.
    let r = app.send(Request::get("/api/openapi.json").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.headers[header::CONTENT_TYPE], "application/json");
    r.json()
}

/// `(METHOD, path) -> operation` of the document.
fn operations(doc: &Value) -> BTreeMap<(String, String), Value> {
    let mut out = BTreeMap::new();
    for (path, item) in doc["paths"].as_object().expect("paths") {
        for method in METHODS {
            if let Some(op) = item.get(*method) {
                out.insert((method.to_ascii_uppercase(), path.clone()), op.clone());
            }
        }
    }
    out
}

fn listed() -> BTreeSet<(String, String)> {
    let set: BTreeSet<_> = ROUTES.iter().map(|(m, p)| (m.to_string(), p.to_string())).collect();
    assert_eq!(set.len(), ROUTES.len(), "ROUTES has duplicates");
    set
}

#[tokio::test]
async fn the_document_is_openapi_3_1_with_info_schemas_and_security() {
    let app = TestApp::new().await;
    let doc = document(&app).await;
    // `FERRY_OPENAPI_OUT=<file> cargo test -p ferry-api --test openapi` writes the document
    // (e.g. to generate client types without running a server).
    if let Some(out) = std::env::var_os("FERRY_OPENAPI_OUT") {
        std::fs::write(out, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    }
    assert!(doc["openapi"].as_str().unwrap().starts_with("3.1."), "{}", doc["openapi"]);
    assert_eq!(doc["info"]["title"], "Ferry API");
    assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));
    let description = doc["info"]["description"].as_str().unwrap();
    assert!(description.contains("Authorization: Bearer") && description.contains("access_token"), "{description}");

    let schemas = doc["components"]["schemas"].as_object().unwrap();
    for name in [
        "ServiceView",
        "Service",
        "Deploy",
        "DeploySource",
        "JobRun",
        "DatastoreView",
        "UpdateDatastore",
        "EnvGroupView",
        "BlueprintResult",
        "ApiErrorBody",
        "ApiErrorDetail",
        "LogLine",
        "ChangeEvent",
        "GithubHookResponse",
    ] {
        assert!(schemas.contains_key(name), "schema {name} missing: {:?}", schemas.keys().collect::<Vec<_>>());
    }
    // ServiceView flattens the stored row into its own fields.
    let view = serde_json::to_string(&schemas["ServiceView"]).unwrap();
    assert!(view.contains("deploy_hook_path") && view.contains("#/components/schemas/Service"), "{view}");
    let change = &schemas["ChangeEvent"];
    for field in ["kind", "id", "service_id", "action"] {
        assert!(change["properties"].get(field).is_some(), "ChangeEvent.{field}: {change}");
    }
    let kinds = serde_json::to_string(&schemas["ChangeKind"]).unwrap();
    assert!(kinds.contains("env_group") && kinds.contains("git_connection"), "{kinds}");

    // The bearer scheme, which Swagger UI's Authorize button fills in, and
    // the dashboard's session cookie.
    let schemes = doc["components"]["securitySchemes"].as_object().unwrap();
    assert_eq!(schemes.keys().collect::<Vec<_>>(), ["bearer", "session"]);
    assert_eq!(schemes["bearer"]["type"], "http");
    assert_eq!(schemes["bearer"]["scheme"], "bearer");
    assert_eq!(schemes["session"]["type"], "apiKey");
    assert_eq!((&schemes["session"]["in"], &schemes["session"]["name"]), (&json!("cookie"), &json!("ferry_session")));

    let tags: Vec<&str> = doc["tags"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(tags, TAGS);
    assert!(doc["tags"].as_array().unwrap().iter().all(|t| t["description"].as_str().is_some_and(|d| !d.is_empty())));

    // Every reference resolves.
    let mut refs = Vec::new();
    collect_refs(&doc, &mut refs);
    assert!(!refs.is_empty());
    for r in refs {
        let name = r.strip_prefix("#/components/schemas/").unwrap_or_else(|| panic!("unexpected $ref {r}"));
        assert!(schemas.contains_key(name), "dangling $ref {r}");
    }
}

fn collect_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get("$ref") {
                out.push(r.clone());
            }
            m.values().for_each(|v| collect_refs(v, out));
        }
        Value::Array(a) => a.iter().for_each(|v| collect_refs(v, out)),
        _ => {}
    }
}

#[tokio::test]
async fn every_route_is_documented_and_nothing_else() {
    let app = TestApp::new().await;
    let doc = document(&app).await;
    let ops = operations(&doc);
    let documented: BTreeSet<_> = ops.keys().cloned().collect();
    let listed = listed();
    let missing: Vec<_> = listed.difference(&documented).collect();
    let extra: Vec<_> = documented.difference(&listed).collect();
    assert!(missing.is_empty(), "routes without documentation: {missing:?}");
    assert!(extra.is_empty(), "documented operations that aren't routes: {extra:?}");

    let mut ids = BTreeSet::new();
    for ((method, path), op) in &ops {
        let what = format!("{method} {path}");
        let id = op["operationId"].as_str().unwrap_or_else(|| panic!("{what}: no operationId"));
        assert!(ids.insert(id.to_string()), "{what}: duplicate operationId {id}");
        assert!(op["summary"].as_str().is_some_and(|s| !s.is_empty()), "{what}: no summary");
        let tags = op["tags"].as_array().unwrap_or_else(|| panic!("{what}: no tag"));
        assert!(tags.len() == 1 && TAGS.contains(&tags[0].as_str().unwrap()), "{what}: tags {tags:?}");

        // Path parameters are documented.
        let declared: BTreeSet<&str> = op["parameters"]
            .as_array()
            .map(|ps| ps.iter().filter(|p| p["in"] == "path").map(|p| p["name"].as_str().unwrap()).collect())
            .unwrap_or_default();
        let in_path: BTreeSet<&str> =
            path.split('/').filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}'))).collect();
        assert_eq!(declared, in_path, "{what}: path parameters");

        // Responses: a success, and ApiErrorBody for every error.
        let responses = op["responses"].as_object().unwrap();
        assert!(responses.keys().any(|s| s.starts_with('2')), "{what}: no success response");
        for (status, resp) in responses {
            if status.starts_with('4') || status.starts_with('5') {
                let schema = &resp["content"]["application/json"]["schema"]["$ref"];
                assert_eq!(schema, "#/components/schemas/ApiErrorBody", "{what}: {status}");
            }
        }

        // The API needs an API token or the dashboard's session (and says 401
        // without); the account itself only takes the session; the rest needs none.
        if path.starts_with("/api/v1/") && !PUBLIC.contains(&path.as_str()) {
            let expected = if path.starts_with("/api/v1/auth/") {
                json!([{"session": []}])
            } else {
                json!([{"bearer": []}, {"session": []}])
            };
            assert_eq!(op["security"], expected, "{what}");
            assert!(responses.contains_key("401"), "{what}: no 401");
            if path.starts_with("/api/v1/auth/") {
                assert!(responses.contains_key("403"), "{what}: no 403 for an API token");
            }
        } else {
            assert!(op.get("security").is_none(), "{what}: {}", op["security"]);
        }
    }
    // no global requirement: operations say it themselves
    assert!(doc.get("security").is_none());
}

#[tokio::test]
async fn streams_and_special_bodies_are_documented() {
    let app = TestApp::new().await;
    let ops = operations(&document(&app).await);
    let op = |m: &str, p: &str| ops[&(m.to_string(), p.to_string())].clone();

    for path in [
        "/api/v1/events",
        "/api/v1/services/{id}/logs",
        "/api/v1/deploys/{deploy_id}/logs",
        "/api/v1/jobs/{job_id}/logs",
    ] {
        let ok = &op("GET", path)["responses"]["200"];
        assert!(ok["content"].get("text/event-stream").is_some(), "{path}: {ok}");
        let description = ok["description"].as_str().unwrap();
        if path == "/api/v1/events" {
            for word in ["event: ready", "event: change", "ChangeEvent", "service_id", "resync"] {
                assert!(description.contains(word), "{path}: {word}");
            }
        } else {
            for word in ["event: log", "LogLine", "event: end"] {
                assert!(description.contains(word), "{path}: {word}");
            }
        }
    }

    let upload = op("POST", "/api/v1/services/{id}/deploys/upload");
    let gzip = &upload["requestBody"]["content"]["application/gzip"]["schema"];
    assert_eq!((gzip["type"].as_str(), gzip["format"].as_str()), (Some("string"), Some("binary")), "{gzip}");
    assert!(upload["responses"].get("413").is_some());

    let apply = op("POST", "/api/v1/blueprints/apply");
    let content = &apply["requestBody"]["content"];
    assert_eq!(content["application/json"]["schema"]["$ref"], "#/components/schemas/ApplyBlueprint");
    assert_eq!(content["application/yaml"]["schema"]["type"], "string");
    assert!(apply["parameters"].as_array().unwrap().iter().any(|p| p["name"] == "dry_run" && p["in"] == "query"));

    let logs = op("GET", "/api/v1/services/{id}/logs");
    let names: BTreeSet<&str> =
        logs["parameters"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, BTreeSet::from(["id", "follow", "tail"]));

    let hook = op("POST", "/hooks/github");
    assert_eq!(
        hook["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/GithubHookResponse"
    );
}

/// The router's answer to an unrouted request: 405, or the JSON 404 of a
/// fallback (the API's, or the reserved-prefix one of the web client).
fn missed_the_router(status: StatusCode, body: &[u8], path: &str) -> bool {
    if status == StatusCode::METHOD_NOT_ALLOWED {
        return true;
    }
    if status != StatusCode::NOT_FOUND {
        return false;
    }
    let message: Value = serde_json::from_slice(body).unwrap_or_default();
    let message = message["error"]["message"].as_str().unwrap_or_default();
    message.starts_with("no API route for") || message == format!("{path} not found")
}

#[tokio::test]
async fn every_listed_route_reaches_a_handler() {
    let app = TestApp::with_config(|c| c.github_webhook_secret = Some("s3cret".into())).await;
    for (method, path) in ROUTES {
        let uri = path
            .replace("{id}", "no-such-thing")
            .replace("{deploy_id}", "dep-nope")
            .replace("{job_id}", "job-nope")
            .replace("{group}", "no-group")
            .replace("{domain}", "example.com")
            .replace("{service_id}", "srv-nope");
        let req = Request::builder()
            .method(Method::from_bytes(method.as_bytes()).unwrap())
            .uri(&uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        // Streams (the change feed) never end: only read error bodies.
        let body = if status.is_success() {
            Vec::new()
        } else {
            resp.into_body().collect().await.unwrap().to_bytes().to_vec()
        };
        assert!(!missed_the_router(status, &body, &uri), "{method} {uri}: {status} {}", String::from_utf8_lossy(&body));
    }

    // ...and the detector really tells misses apart.
    for (method, uri) in
        [(Method::GET, "/api/v1/nope"), (Method::PUT, "/api/v1/services"), (Method::GET, "/hooks/nope")]
    {
        let r = app
            .send(
                Request::builder()
                    .method(method.clone())
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert!(missed_the_router(r.status, &r.body, uri), "{method} {uri}: {} {}", r.status, r.text());
    }
}

#[tokio::test]
async fn resource_limits_are_documented() {
    let app = TestApp::new().await;
    let doc = document(&app).await;
    let ops = operations(&doc);
    let op = |m: &str, p: &str| ops[&(m.to_string(), p.to_string())].clone();
    let schemas = &doc["components"]["schemas"];

    // PATCH /api/v1/datastores/{id} takes an UpdateDatastore
    let patch = op("PATCH", "/api/v1/datastores/{id}");
    assert_eq!(patch["operationId"], "updateDatastore");
    assert_eq!(
        patch["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/UpdateDatastore"
    );
    assert_eq!(
        patch["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/DatastoreView"
    );
    for status in ["400", "404", "409"] {
        assert!(patch["responses"].get(status).is_some(), "{status}: {patch}");
    }
    let description = patch["description"].as_str().unwrap();
    assert!(description.contains("without a restart") && description.contains("`0`"), "{description}");
    for field in ["memory_limit_mb", "cpu_limit"] {
        for schema in ["UpdateDatastore", "CreateDatastore", "CreateService", "UpdateService", "Service", "Datastore"] {
            assert!(schemas[schema]["properties"].get(field).is_some(), "{schema}.{field}");
        }
    }
    for field in ["default_memory_limit_mb", "default_cpu_limit", "docker_cpus", "docker_memory_bytes"] {
        assert!(schemas["ServerInfo"]["properties"].get(field).is_some(), "ServerInfo.{field}");
    }

    // services: limits are saved by PATCH, applied by the next deploy
    let description = op("PATCH", "/api/v1/services/{id}")["description"].as_str().unwrap().to_string();
    assert!(description.contains("take effect with the next deploy or restart"), "{description}");
    assert!(description.contains("doesn't redeploy"), "{description}");
    let description = op("POST", "/api/v1/services")["description"].as_str().unwrap().to_string();
    assert!(description.contains("memory_limit_mb") && description.contains("server default"), "{description}");
    let description = op("POST", "/api/v1/datastores")["description"].as_str().unwrap().to_string();
    assert!(description.contains("memory_limit_mb") && description.contains("server default"), "{description}");
}

#[tokio::test]
async fn accounts_are_documented_without_their_secrets() {
    let app = TestApp::new().await;
    let doc = document(&app).await;
    let ops = operations(&doc);
    let op = |m: &str, p: &str| ops[&(m.to_string(), p.to_string())].clone();
    let schemas = &doc["components"]["schemas"];
    let body = |op: &Value, status: &str| op["responses"][status]["content"]["application/json"]["schema"].clone();
    let request = |op: &Value| op["requestBody"]["content"]["application/json"]["schema"]["$ref"].clone();

    for (method, path, takes, status, gives) in [
        ("POST", "/api/v1/auth/setup", "SetupAccount", "201", "AuthStatus"),
        ("POST", "/api/v1/auth/login", "Login", "200", "AuthStatus"),
        ("POST", "/api/v1/auth/tokens", "CreateApiToken", "201", "CreatedApiToken"),
        ("POST", "/api/v1/auth/cli", "StartCliLogin", "201", "CliLoginStarted"),
        ("POST", "/api/v1/auth/cli/{id}/token", "CliLoginPoll", "200", "CliLoginResult"),
    ] {
        let operation = op(method, path);
        assert_eq!(request(&operation), format!("#/components/schemas/{takes}"), "{path}");
        assert_eq!(body(&operation, status)["$ref"], format!("#/components/schemas/{gives}"), "{path}");
    }
    assert_eq!(body(&op("GET", "/api/v1/auth/status"), "200")["$ref"], "#/components/schemas/AuthStatus");
    assert_eq!(schemas["AuthKind"]["enum"], json!(["session", "token"]));
    assert_eq!(schemas["CliLoginStatus"]["enum"], json!(["pending", "approved", "denied"]));
    assert_eq!(schemas["SetupAccount"]["required"], json!(["email", "password", "code"]));
    for status in ["401", "429"] {
        assert!(op("POST", "/api/v1/auth/login")["responses"].get(status).is_some(), "{status}");
    }
    for status in ["403", "409", "429"] {
        assert!(op("POST", "/api/v1/auth/setup")["responses"].get(status).is_some(), "{status}");
    }

    // What the API says about an account, a session or a token is never a secret.
    let fields = |schema: &str| schemas[schema]["properties"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(fields("UserView"), ["created_at", "email", "id"]);
    for view in ["SessionView", "ApiTokenView", "CliLoginView"] {
        for secret in ["token", "token_hash", "secret", "password_hash"] {
            assert!(!fields(view).iter().any(|f| f == secret), "{view}.{secret}");
        }
    }
    // The token itself only where it is handed over.
    assert!(fields("CreatedApiToken").iter().any(|f| f == "token"));
    assert!(fields("CliLoginResult").iter().any(|f| f == "token"));
}

#[tokio::test]
async fn git_connections_are_documented_without_their_secrets() {
    let app = TestApp::new().await;
    let doc = document(&app).await;
    let ops = operations(&doc);
    let op = |m: &str, p: &str| ops[&(m.to_string(), p.to_string())].clone();
    let schemas = &doc["components"]["schemas"];
    let body = |op: &Value, status: &str| op["responses"][status]["content"]["application/json"]["schema"].clone();
    let request = |op: &Value| op["requestBody"]["content"]["application/json"]["schema"]["$ref"].clone();

    // Authorizing in the browser: where to send it, then what it came back with.
    let authorize = op("POST", "/api/v1/git/authorize");
    let callback = op("POST", "/api/v1/git/callback");
    assert_eq!((&authorize["operationId"], &callback["operationId"]), (&json!("authorizeGit"), &json!("gitCallback")));
    assert_eq!(request(&authorize), "#/components/schemas/AuthorizeGit");
    assert_eq!(request(&callback), "#/components/schemas/GitCallback");
    for operation in [&authorize, &callback] {
        assert_eq!(body(operation, "200")["$ref"], "#/components/schemas/GitAuthorization");
        for status in ["400", "502"] {
            assert!(operation["responses"].get(status).is_some(), "{status}: {operation}");
        }
    }
    assert_eq!(schemas["AuthorizeGit"]["required"], json!(["redirect_uri"]));
    assert_eq!(schemas["GitCallback"]["required"], json!(["state"]));
    assert_eq!(schemas["GitAuthorizationStatus"]["enum"], json!(["redirect", "connected"]));
    assert_eq!(schemas["GitRedirectMethod"]["enum"], json!(["get", "post"]));
    assert_eq!(schemas["GitAuth"]["enum"], json!(["github_app", "oauth", "token"]));
    assert_eq!(schemas["GitConnectionStatus"]["enum"], json!(["connected", "pending"]));

    // Connecting with a token takes one; nothing ever returns a secret.
    let connect = op("POST", "/api/v1/git/connections");
    assert_eq!(connect["operationId"], "connectGit");
    assert_eq!(
        connect["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ConnectGit"
    );
    for status in ["200", "201"] {
        assert_eq!(body(&connect, status)["$ref"], "#/components/schemas/GitConnectionView", "{status}");
    }
    for status in ["400", "502"] {
        assert!(connect["responses"].get(status).is_some(), "{status}: {connect}");
    }
    assert!(schemas["ConnectGit"]["properties"].get("token").is_some());
    let view = schemas["GitConnectionView"]["properties"].as_object().unwrap();
    for shown in ["auth", "status", "token_hint", "client_id", "app_slug", "manage_url", "services"] {
        assert!(view.contains_key(shown), "{shown}: {view:?}");
    }
    for secret in ["token", "refresh_token", "client_secret", "private_key", "webhook_secret"] {
        assert!(!view.contains_key(secret), "{secret}: {view:?}");
    }
    assert_eq!(schemas["GitProvider"]["enum"], json!(["github", "gitlab"]));

    let repositories = op("GET", "/api/v1/git/connections/{id}/repositories");
    assert_eq!(body(&repositories, "200")["$ref"], "#/components/schemas/GitRepositoryList");
    for status in ["404", "409", "502"] {
        assert!(repositories["responses"].get(status).is_some(), "{status}: {repositories}");
    }
    // Branches are asked by repository URL, whatever reads it.
    let branches = op("GET", "/api/v1/git/branches");
    assert_eq!(body(&branches, "200")["$ref"], "#/components/schemas/GitBranches");
    let repo_url = branches["parameters"].as_array().unwrap().iter().find(|p| p["name"] == "repo_url").unwrap();
    assert_eq!((&repo_url["in"], &repo_url["required"]), (&json!("query"), &json!(true)));
    for status in ["400", "502"] {
        assert!(branches["responses"].get(status).is_some(), "{status}: {branches}");
    }
    let delete = op("DELETE", "/api/v1/git/connections/{id}");
    assert!(delete["parameters"].as_array().unwrap().iter().any(|p| p["name"] == "force" && p["in"] == "query"));

    // Connections belong to the server: services name none.
    for schema in ["Service", "CreateService", "UpdateService"] {
        assert!(schemas[schema]["properties"].get("git_connection_id").is_none(), "{schema}.git_connection_id");
    }
}

/// `.route("<path>", <method routers>)` calls of `src/lib.rs` (each on one
/// line), with the API router's paths under `/api`.
fn routes_in_lib_rs() -> BTreeSet<(String, String)> {
    let src = include_str!("../src/lib.rs");
    let mut out = BTreeSet::new();
    for line in src.lines().map(str::trim).filter(|l| l.contains(".route(")) {
        let rest = line
            .strip_prefix(".route(\"")
            .unwrap_or_else(|| panic!("keep every `.route(\"<path>\", ...)` call of lib.rs on one line: {line}"));
        let (path, handlers) = rest.split_once('"').unwrap();
        let path = if path.starts_with("/v1/") { format!("/api{path}") } else { path.to_string() };
        let mut methods = 0;
        for (token, method) in
            [("get(", "GET"), ("post(", "POST"), ("put(", "PUT"), ("patch(", "PATCH"), ("delete(", "DELETE")]
        {
            if handlers.contains(token) {
                out.insert((method.to_string(), path.clone()));
                methods += 1;
            }
        }
        assert!(methods > 0, "no method router found in: {line}");
    }
    out
}

#[test]
fn every_route_of_lib_rs_is_listed() {
    let in_source = routes_in_lib_rs();
    let mut expected = listed();
    for (m, p) in NOT_IN_LIB_RS {
        assert!(expected.remove(&(m.to_string(), p.to_string())), "{m} {p}");
    }
    let unlisted: Vec<_> = in_source.difference(&expected).collect();
    let stale: Vec<_> = expected.difference(&in_source).collect();
    assert!(unlisted.is_empty(), "routes missing from ROUTES (document them too): {unlisted:?}");
    assert!(stale.is_empty(), "ROUTES entries that lib.rs doesn't route: {stale:?}");
}

#[tokio::test]
async fn swagger_ui_is_served_without_a_token() {
    let app = TestApp::new().await;
    let get = |uri: &str| app.send(Request::get(uri).body(Body::empty()).unwrap());

    // `/api/docs` → `/api/docs/` (the page loads its assets relatively).
    let r = get("/api/docs").await;
    assert!(r.status.is_redirection(), "{}", r.status);
    assert_eq!(r.headers[header::LOCATION], "/api/docs/");

    let r = get("/api/docs/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
    assert_eq!(r.headers[header::X_FRAME_OPTIONS], "DENY");
    let html = r.text();
    assert!(html.contains("swagger-ui") && html.contains("swagger-initializer.js"), "{html}");

    // Configured for this document, and offline (no validator.swagger.io).
    let r = get("/api/docs/swagger-initializer.js").await;
    assert_eq!(r.status, StatusCode::OK);
    let init = r.text();
    assert!(init.contains("\"/api/openapi.json\""), "{init}");
    assert!(init.contains("\"validatorUrl\": \"none\""), "{init}");

    // The assets are compiled in (vendored), not fetched from a CDN.
    for asset in ["swagger-ui-bundle.js", "swagger-ui.css", "swagger-ui-standalone-preset.js"] {
        let r = get(&format!("/api/docs/{asset}")).await;
        assert_eq!(r.status, StatusCode::OK, "{asset}");
        assert!(r.body.len() > 1000, "{asset}");
    }
    assert!(!html.contains("//cdn") && !html.contains("unpkg.com"), "{html}");

    // A wrong token doesn't matter either (nothing is checked)...
    let r = app
        .send(
            Request::get("/api/openapi.json").header(header::AUTHORIZATION, "Bearer nope").body(Body::empty()).unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    // ...while the API itself still requires it, and the fallbacks still answer around the docs.
    assert_eq!(get("/api/v1/info").await.status, StatusCode::UNAUTHORIZED);
    for path in ["/api/openapi.yaml", "/api/doc", "/api/docsx"] {
        let r = get(path).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}: unknown /api paths stay behind the token");
    }
    let r = get("/services/api/docs").await;
    assert_eq!(r.status, StatusCode::OK, "SPA routes are untouched");
    assert!(r.headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
    let r = app.send(Request::post("/api/docs/").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.code(), "method_not_allowed");
}

#[tokio::test]
async fn the_documented_bearer_scheme_authenticates() {
    // What Swagger UI sends once a token is entered in Authorize.
    let app = TestApp::new().await;
    let doc = document(&app).await;
    let scheme = doc["components"]["securitySchemes"]["bearer"]["scheme"].as_str().unwrap();
    let r = app
        .send(
            Request::get("/api/v1/info")
                .header(header::AUTHORIZATION, format!("{} {TOKEN}", capitalize(scheme)))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}
