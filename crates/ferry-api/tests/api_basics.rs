//! Auth, liveness, dashboard, fallbacks and server info.

mod common;

use axum::body::Body;
use common::{TOKEN, TestApp};
use http::{Method, Request, StatusCode};
use serde_json::json;

fn req(method: Method, uri: &str) -> http::request::Builder {
    Request::builder().method(method).uri(uri)
}

#[tokio::test]
async fn healthz_and_dashboard_need_no_auth() {
    let app = TestApp::new().await;
    let r = app.send(req(Method::GET, "/healthz").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.text(), "ok");

    for path in ["/", "/index.html"] {
        let r = app.send(req(Method::GET, path).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert!(r.headers["content-type"].to_str().unwrap().starts_with("text/html"));
        assert_eq!(r.headers["cache-control"], "no-cache");
        assert_eq!(r.headers["x-frame-options"], "DENY");
        assert_eq!(r.text(), ferry_api::DASHBOARD_HTML);
    }
}

#[tokio::test]
async fn api_requires_a_valid_token() {
    let app = TestApp::new().await;
    // missing
    let r = app.send(req(Method::GET, "/api/v1/info").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.code(), "unauthorized");
    // wrong bearer
    let r = app
        .send(req(Method::GET, "/api/v1/info").header("authorization", "Bearer nope").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // wrong scheme
    let r = app
        .send(
            req(Method::GET, "/api/v1/info")
                .header("authorization", format!("Basic {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // right bearer (scheme is case-insensitive)
    let r = app
        .send(
            req(Method::GET, "/api/v1/info")
                .header("authorization", format!("bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    // access_token on GET
    let r =
        app.send(req(Method::GET, &format!("/api/v1/info?access_token={TOKEN}")).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = app.send(req(Method::GET, "/api/v1/info?access_token=wrong").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // access_token is not accepted for other methods
    let r = app
        .send(
            req(Method::POST, &format!("/api/v1/env-groups?access_token={TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"g"}"#))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(app.store.list_env_groups().await.unwrap().is_empty());
}

#[tokio::test]
async fn empty_server_token_fails_closed() {
    let app = TestApp::with_config(|c| c.api_token = String::new()).await;
    let r = app
        .send(req(Method::GET, "/api/v1/info").header("authorization", "Bearer ").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_routes_are_json_404s() {
    let app = TestApp::new().await;
    let r = app.get("/api/v1/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.code(), "not_found");
    for path in ["/api", "/api/", "/api/v1", "/api/v2/services"] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(r.code(), "not_found", "{path}");
    }
    let r = app.get("/api/whatever/deep/path").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.code(), "not_found");
    // unauthenticated unknown API path: auth runs first
    let r = app.send(req(Method::GET, "/api/v1/nope").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // outside /api
    let r = app.send(req(Method::GET, "/nope").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.code(), "not_found");
    // wrong method on a known route
    let r = app.call(Method::PUT, "/api/v1/services", Some(json!({}))).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.code(), "method_not_allowed");
}

#[tokio::test]
async fn extractor_rejections_are_json() {
    let app = TestApp::new().await;
    let r = app
        .send(
            req(Method::POST, "/api/v1/services")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.code(), "invalid_request");
    // wrong field type
    let r = app.post("/api/v1/services", json!({"name": "web", "instances": "many"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // missing required field
    let r = app.post("/api/v1/services", json!({})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("name"));
    // bad query
    let r = app.get("/api/v1/services/x/deploys?limit=abc").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.code(), "invalid_request");
    // bad enum
    let r = app.post("/api/v1/services", json!({"name": "web", "type": "mainframe"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn server_info() {
    let app = TestApp::with_config(|c| {
        c.github_webhook_secret = Some("s3cret".into());
        c.proxy_addr = "0.0.0.0:8080".parse().unwrap();
    })
    .await;
    let r = app.get("/api/v1/info").await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["version"], ferry_core::VERSION);
    assert_eq!(v["base_domain"], "localhost");
    assert_eq!(v["proxy_url"], "http://localhost:8080");
    assert_eq!(v["tls_enabled"], false);
    assert_eq!(v["dashboard_url"], "http://ferry.localhost:8080");
    assert_eq!(v["github_webhook_enabled"], true);
    assert_eq!(v["docker_version"], "27.0.0");

    let app = TestApp::new().await;
    assert_eq!(app.get("/api/v1/info").await.json()["github_webhook_enabled"], false);
}
