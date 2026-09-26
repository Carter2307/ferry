//! `GET /api/v1/events`: the SSE change feed of the web client.

mod common;

use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use common::{TOKEN, TestApp};
use ferry_core::DeployStatus;
use futures::StreamExt;
use http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

/// An open change feed.
struct Feed {
    body: BodyDataStream,
    buf: String,
}

impl Feed {
    async fn open(app: &TestApp) -> Feed {
        let req = Request::builder()
            .uri("/api/v1/events")
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        assert_eq!(resp.headers()["cache-control"], "no-cache");
        let mut feed = Feed { body: resp.into_body().into_data_stream(), buf: String::new() };
        let (event, data) = feed.next_event().await.expect("a ready event");
        assert_eq!((event.as_str(), data.as_str()), ("ready", "{}"));
        feed
    }

    /// The next `(event, data)` (comments skipped); `None` when the stream ended.
    async fn next_event(&mut self) -> Option<(String, String)> {
        self.next_event_or(|| "an event".to_string()).await
    }

    async fn next_event_or(&mut self, waiting_for: impl Fn() -> String) -> Option<(String, String)> {
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let frame: String = self.buf.drain(..end + 2).collect();
                let mut event = String::new();
                let mut data = String::new();
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("event: ") {
                        event = v.to_string();
                    } else if let Some(v) = line.strip_prefix("data: ") {
                        data = v.to_string();
                    }
                }
                if event.is_empty() {
                    continue; // keep-alive comment
                }
                return Some((event, data));
            }
            let chunk = tokio::time::timeout(Duration::from_secs(5), self.body.next())
                .await
                .unwrap_or_else(|_| panic!("no event within 5s while waiting for {}", waiting_for()))?
                .unwrap();
            self.buf.push_str(std::str::from_utf8(&chunk).unwrap());
        }
    }

    /// Changes until one matches `(kind, id, action)`; returns everything seen.
    async fn until(&mut self, kind: &str, id: &str, action: &str) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let (event, data) =
                self.next_event_or(|| format!("{kind} {id} {action} (seen: {seen:?})")).await.expect("the feed ended");
            assert_eq!(event, "change", "{data}");
            let change: Value = serde_json::from_str(&data).unwrap();
            seen.push(change.clone());
            if change["kind"] == kind && change["id"] == id && change["action"] == action {
                return seen;
            }
        }
    }
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn api_writes_are_reported_as_changes() {
    let app = TestApp::new().await;
    let mut feed = Feed::open(&app).await;

    // a service with an image: the service and its first deploy
    let web = app.create_service(json!({"name": "web", "image": "nginx"})).await;
    let web_id = id(&web);
    let deploy_id = web["latest_deploy"]["id"].as_str().unwrap().to_string();
    let seen = feed.until("service", &web_id, "created").await;
    assert_eq!(
        seen.last().unwrap(),
        &json!({"kind": "service", "id": web_id, "service_id": web_id, "action": "created"})
    );
    let seen = feed.until("deploy", &deploy_id, "created").await;
    assert_eq!(seen.last().unwrap()["service_id"], web_id.as_str());

    // settings, env vars
    app.patch("/api/v1/services/web", json!({"health_check_path": "/h"})).await;
    feed.until("service", &web_id, "updated").await;
    app.patch("/api/v1/services/web/env", json!({"set": [{"key": "A", "value": "1"}]})).await;
    feed.until("service", &web_id, "updated").await;

    // env groups: created, linked (both sides change), vars changed
    let group = app.post("/api/v1/env-groups", json!({"name": "shared"})).await.json();
    let group_id = id(&group);
    let seen = feed.until("env_group", &group_id, "created").await;
    assert_eq!(seen.last().unwrap()["service_id"], Value::Null);
    app.post("/api/v1/services/web/env-groups", json!({"group": "shared"})).await;
    let seen = feed.until("service", &web_id, "updated").await;
    assert!(seen.iter().any(|c| c["kind"] == "env_group" && c["id"] == group_id.as_str()), "{seen:?}");
    app.put("/api/v1/env-groups/shared/env", json!({"vars": [{"key": "X", "value": "1"}]})).await;
    feed.until("env_group", &group_id, "updated").await;

    // datastores
    let ds = app.post("/api/v1/datastores", json!({"name": "cache", "kind": "redis"})).await.json();
    feed.until("datastore", &id(&ds), "created").await;

    // deploy status changes, jobs
    app.post(&format!("/api/v1/deploys/{deploy_id}/cancel"), json!({})).await;
    feed.until("deploy", &deploy_id, "updated").await;
    let job = app.post("/api/v1/services/web/jobs", json!({"command": "echo hi"})).await.json();
    let job_id = id(&job);
    let seen = feed.until("job", &job_id, "created").await;
    assert_eq!(seen.last().unwrap()["service_id"], web_id.as_str());
    app.post(&format!("/api/v1/jobs/{job_id}/cancel"), json!({})).await;
    feed.until("job", &job_id, "updated").await;

    // deletions
    app.delete("/api/v1/datastores/cache").await;
    feed.until("datastore", &id(&ds), "deleted").await;
    app.delete("/api/v1/env-groups/shared?force=true").await;
    feed.until("env_group", &group_id, "deleted").await;
    assert_eq!(app.delete("/api/v1/services/web").await.status, StatusCode::NO_CONTENT);
    feed.until("service", &web_id, "deleted").await;
}

#[tokio::test]
async fn engine_side_changes_are_found_by_polling() {
    let app = TestApp::new().await;
    let web = app.create_service(json!({"name": "web", "image": "nginx"})).await;
    let deploy_id = web["latest_deploy"]["id"].as_str().unwrap().to_string();
    let mut feed = Feed::open(&app).await;
    // The engine writes the store directly (no API request, no nudge).
    app.store.set_deploy_status(&deploy_id, DeployStatus::Building, None).await.unwrap();
    let seen = feed.until("deploy", &deploy_id, "updated").await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    app.store.set_live_deploy(&id(&web), Some(&deploy_id)).await.unwrap();
    feed.until("service", &id(&web), "updated").await;
}

#[tokio::test]
async fn every_subscriber_gets_every_change() {
    let app = TestApp::new().await;
    let mut a = Feed::open(&app).await;
    let mut b = Feed::open(&app).await;
    let v = app.create_service(json!({"name": "web"})).await;
    a.until("service", &id(&v), "created").await;
    b.until("service", &id(&v), "created").await;
    // one subscriber leaving doesn't stop the other's feed
    drop(a);
    app.patch("/api/v1/services/web", json!({"health_check_path": "/h"})).await;
    b.until("service", &id(&v), "updated").await;
}

#[tokio::test]
async fn the_feed_needs_a_token_and_accepts_it_in_the_query() {
    let app = TestApp::new().await;
    let r = app.send(Request::builder().uri("/api/v1/events").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.code(), "unauthorized");
    let req = Request::builder().uri(format!("/api/v1/events?access_token={TOKEN}")).body(Body::empty()).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let r = app.call(Method::POST, "/api/v1/events", None).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn shutdown_ends_the_feed() {
    let app = TestApp::new().await;
    let mut feed = Feed::open(&app).await;
    app.shutdown.cancel();
    let end = tokio::time::timeout(Duration::from_secs(2), feed.next_event()).await.expect("the feed must end");
    assert!(end.is_none(), "{end:?}");
    // new subscribers after shutdown end right away too
    let req = Request::builder()
        .uri("/api/v1/events")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let mut body = resp.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), body.next()).await.expect("the feed must end");
    assert!(first.is_none(), "{first:?}");
}
