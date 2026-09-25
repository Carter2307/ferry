//! Deploy hooks and GitHub webhooks.

mod common;

use axum::body::Body;
use common::TestApp;
use hmac::{Hmac, Mac};
use http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use sha2::Sha256;

const SECRET: &str = "gh-webhook-secret";

#[tokio::test]
async fn deploy_hook_needs_the_right_key() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "image": "nginx", "deploy": false})).await;
    let path = v["deploy_hook_path"].as_str().unwrap().to_string();
    let id = v["id"].as_str().unwrap().to_string();

    for method in [Method::POST, Method::GET] {
        let r = app.send(Request::builder().method(method).uri(&path).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
        assert_eq!(r.json()["trigger"], "deploy_hook");
    }
    assert_eq!(app.engine.calls_with("deploy ").len(), 2);
    assert!(app.engine.calls_with("deploy ")[0].starts_with(&format!("deploy {id} deploy_hook")));

    let bad =
        [format!("/hooks/deploy/{id}?key=wrong"), format!("/hooks/deploy/{id}?key="), format!("/hooks/deploy/{id}")];
    for uri in bad {
        let r = app.send(Request::builder().method(Method::POST).uri(&uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(r.code(), "unauthorized");
    }
    let r = app
        .send(Request::builder().method(Method::POST).uri("/hooks/deploy/srv-nope?key=x").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(app.engine.calls_with("deploy ").len(), 2);
}

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn github_req(event: &str, body: &Value, signature: Option<String>) -> Request<Body> {
    let bytes = serde_json::to_vec(body).unwrap();
    let sig = signature.unwrap_or_else(|| sign(SECRET, &bytes));
    Request::builder()
        .method(Method::POST)
        .uri("/hooks/github")
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-hub-signature-256", sig)
        .body(Body::from(bytes))
        .unwrap()
}

fn push(branch: &str, deleted: bool) -> Value {
    json!({
        "ref": format!("refs/heads/{branch}"),
        "before": "1111111111111111111111111111111111111111",
        "after": if deleted { "0000000000000000000000000000000000000000" } else { "abcdef0123456789abcdef0123456789abcdef01" },
        "deleted": deleted,
        "repository": {
            "full_name": "Acme/App",
            "html_url": "https://github.com/Acme/App",
            "clone_url": "https://github.com/Acme/App.git",
            "ssh_url": "git@github.com:Acme/App.git",
            "git_url": "git://github.com/Acme/App.git",
            "url": "https://github.com/Acme/App"
        }
    })
}

async fn app_with_secret() -> TestApp {
    TestApp::with_config(|c| c.github_webhook_secret = Some(SECRET.into())).await
}

#[tokio::test]
async fn github_disabled_without_secret() {
    let app = TestApp::new().await;
    let r = app.send(github_req("ping", &json!({}), None)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.code(), "not_found");
}

#[tokio::test]
async fn github_signature_is_verified() {
    let app = app_with_secret().await;
    let body = json!({"zen": "hi"});
    let r = app.send(github_req("ping", &body, None)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({"ok": true}));

    let bytes = serde_json::to_vec(&body).unwrap();
    for sig in [sign("wrong", &bytes), "sha256=00".into(), "garbage".into(), sign(SECRET, b"other body")] {
        let r = app.send(github_req("ping", &body, Some(sig))).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    // missing header
    let r = app
        .send(
            Request::builder()
                .method(Method::POST)
                .uri("/hooks/github")
                .header("x-github-event", "ping")
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn github_push_deploys_matching_services() {
    let app = app_with_secret().await;
    let https = app
        .create_service(json!({"name": "https-svc", "repo_url": "https://github.com/acme/app", "deploy": false}))
        .await;
    let ssh = app
        .create_service(json!({"name": "ssh-svc", "repo_url": "git@github.com:acme/app.git", "deploy": false}))
        .await;
    app.create_service(
        json!({"name": "dev-branch", "repo_url": "https://github.com/acme/app", "branch": "dev", "deploy": false}),
    )
    .await;
    app.create_service(
        json!({"name": "manual", "repo_url": "https://github.com/acme/app", "auto_deploy": false, "deploy": false}),
    )
    .await;
    app.create_service(json!({"name": "other-repo", "repo_url": "https://github.com/acme/other", "deploy": false}))
        .await;
    let suspended =
        app.create_service(json!({"name": "sleepy", "repo_url": "https://github.com/acme/app", "deploy": false})).await;
    app.store.set_suspended(suspended["id"].as_str().unwrap(), true).await.unwrap();
    app.create_service(json!({"name": "image-svc", "image": "nginx", "deploy": false})).await;

    let r = app.send(github_req("push", &push("main", false), None)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let deploys = r.json()["deploys"].as_array().unwrap().clone();
    let mut ids: Vec<&str> = deploys.iter().map(|d| d["service_id"].as_str().unwrap()).collect();
    ids.sort();
    let mut want = vec![https["id"].as_str().unwrap(), ssh["id"].as_str().unwrap()];
    want.sort();
    assert_eq!(ids, want);
    for d in &deploys {
        assert_eq!(d["trigger"], "webhook");
        assert_eq!(d["commit_sha"], "abcdef0123456789abcdef0123456789abcdef01");
    }

    // another branch
    app.engine.clear();
    let r = app.send(github_req("push", &push("dev", false), None)).await;
    assert_eq!(r.json()["deploys"].as_array().unwrap().len(), 1);

    // no service on that branch
    let r = app.send(github_req("push", &push("feature/x", false), None)).await;
    assert_eq!(r.json()["deploys"], json!([]));

    // deleted branch → ignored
    app.engine.clear();
    let r = app.send(github_req("push", &push("main", true), None)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["deploys"], json!([]));
    assert!(app.engine.calls().is_empty());

    // tags are not branches
    let mut tag = push("main", false);
    tag["ref"] = json!("refs/tags/v1.0.0");
    let r = app.send(github_req("push", &tag, None)).await;
    assert_eq!(r.json()["deploys"], json!([]));
    assert!(app.engine.calls().is_empty());
}

#[tokio::test]
async fn github_other_events_and_bad_payloads() {
    let app = app_with_secret().await;
    let r = app.send(github_req("issues", &json!({"action": "opened"}), None)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({"ignored": true}));

    // signed but not JSON
    let body = b"not json".to_vec();
    let r = app
        .send(
            Request::builder()
                .method(Method::POST)
                .uri("/hooks/github")
                .header("x-github-event", "push")
                .header("x-hub-signature-256", sign(SECRET, &body))
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // push without repository URLs
    let r = app.send(github_req("push", &json!({"ref": "refs/heads/main", "after": "abc"}), None)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}
