//! Services: CRUD, lifecycle actions, deploys, uploads and log streams.

mod common;

use std::io::Write;

use axum::body::Body;
use common::{TOKEN, TestApp, state_of};
use ferry_core::{DeployStatus, ServiceState};
use http::{Method, Request, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn create_with_image_queues_a_create_deploy() {
    let app = TestApp::new().await;
    let v = app
        .create_service(json!({
            "name": "web",
            "image": "nginx:alpine",
            "port": 80,
            "custom_domains": ["App.Example.com."],
            "env": [{"key": "A", "value": "1"}],
        }))
        .await;
    assert_eq!(v["name"], "web");
    assert_eq!(v["type"], "web_service");
    assert_eq!(v["runtime"], "image");
    assert_eq!(v["url"], "http://web.localhost:8080");
    assert_eq!(v["hosts"], json!(["web.localhost", "app.example.com"]));
    assert_eq!(v["internal_host"], "web");
    assert_eq!(v["internal_port"], 80);
    assert_eq!(v["custom_domains"], json!(["app.example.com"]));
    let id = v["id"].as_str().unwrap();
    let key = v["deploy_hook_key"].as_str().unwrap();
    assert_eq!(v["deploy_hook_path"], format!("/hooks/deploy/{id}?key={key}"));
    assert_eq!(state_of(&v), ServiceState::Deploying);
    assert_eq!(v["latest_deploy"]["trigger"], "create");
    assert_eq!(
        app.engine.calls_with("deploy "),
        vec![format!("deploy {id} create commit=- clear_cache=false source=-")]
    );
    assert_eq!(app.get("/api/v1/services/web/env").await.json(), json!([{"key": "A", "value": "1"}]));
}

#[tokio::test]
async fn create_without_source_or_with_deploy_false_does_not_deploy() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "up-only"})).await;
    assert_eq!(state_of(&v), ServiceState::NotDeployed);
    assert_eq!(v["runtime"], "auto");
    let v = app.create_service(json!({"name": "later", "repo_url": "https://github.com/a/b", "deploy": false})).await;
    assert_eq!(state_of(&v), ServiceState::NotDeployed);
    assert!(app.engine.calls_with("deploy ").is_empty());

    let v = app.create_service(json!({"name": "git", "repo_url": "https://github.com/a/b", "branch": "dev"})).await;
    assert_eq!(v["branch"], "dev");
    assert_eq!(app.engine.calls_with("deploy ").len(), 1);
    assert_eq!(v["latest_deploy"]["source"]["kind"], "git");
}

#[tokio::test]
async fn create_validation_errors() {
    let app = TestApp::new().await;
    let cases = [
        json!({"name": "Bad Name"}),
        json!({"name": "ferry"}),
        json!({"name": "x", "repo_url": "https://a/b", "image": "nginx"}),
        json!({"name": "x", "type": "cron"}),
        json!({"name": "x", "type": "worker", "custom_domains": ["a.example.com"]}),
        json!({"name": "x", "custom_domains": ["not a domain"]}),
        json!({"name": "x", "instances": 0}),
        json!({"name": "x", "instances": 51}),
        json!({"name": "x", "disk_mount_path": "/data", "instances": 2}),
        json!({"name": "x", "health_check_path": "healthz"}),
        json!({"name": "x", "env": [{"key": "BAD KEY", "value": "1"}]}),
        json!({"name": "x", "env_groups": ["missing"]}),
        json!({"name": "x", "runtime": "image"}),
    ];
    for body in cases {
        let r = app.post("/api/v1/services", body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
        assert_eq!(r.code(), "invalid_request");
    }
    assert!(app.store.list_services().await.unwrap().is_empty());
    assert!(app.engine.calls().is_empty());
}

#[tokio::test]
async fn create_conflicts() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web", "custom_domains": ["app.example.com"]})).await;
    // duplicate name
    let r = app.post("/api/v1/services", json!({"name": "web"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // name taken by a datastore
    let r = app.post("/api/v1/datastores", json!({"name": "db", "kind": "postgres"})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = app.post("/api/v1/services", json!({"name": "db"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // domain used by another service
    let r = app.post("/api/v1/services", json!({"name": "two", "custom_domains": ["APP.example.com"]})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // domain equal to another service's default host
    let r = app.post("/api/v1/services", json!({"name": "three", "custom_domains": ["web.localhost"]})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // a new service whose default host is someone's custom domain
    app.create_service(json!({"name": "four", "custom_domains": ["five.localhost"]})).await;
    let r = app.post("/api/v1/services", json!({"name": "five"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // dashboard host
    let r = app.post("/api/v1/services", json!({"name": "six", "custom_domains": ["ferry.localhost"]})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn create_links_env_groups_in_order() {
    let app = TestApp::new().await;
    for g in ["b-group", "a-group"] {
        assert_eq!(app.post("/api/v1/env-groups", json!({"name": g})).await.status, StatusCode::CREATED);
    }
    let v = app.create_service(json!({"name": "web", "env_groups": ["b-group", "a-group", "b-group"]})).await;
    assert_eq!(v["env_groups"], json!(["b-group", "a-group"]));
}

#[tokio::test]
async fn list_get_by_id_or_name_and_404() {
    let app = TestApp::new().await;
    let a = app.create_service(json!({"name": "b-svc", "type": "worker"})).await;
    app.create_service(json!({"name": "a-svc", "type": "pserv"})).await;
    let list = app.get("/api/v1/services").await.json();
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["a-svc", "b-svc"]);
    let by_id = app.get(&format!("/api/v1/services/{}", a["id"].as_str().unwrap())).await.json();
    assert_eq!(by_id["name"], "b-svc");
    assert_eq!(by_id["url"], Value::Null);
    assert_eq!(by_id["hosts"], json!([]));
    let r = app.get("/api/v1/services/missing").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.code(), "not_found");
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("missing"));
}

#[tokio::test]
async fn internal_port_falls_back_to_the_live_deploy() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "backend", "type": "pserv"})).await;
    assert_eq!(app.get("/api/v1/services/backend").await.json()["internal_port"], Value::Null);
    app.make_live("backend", Some(3000)).await;
    let v = app.get("/api/v1/services/backend").await.json();
    assert_eq!(v["internal_port"], 3000);
    assert_eq!(state_of(&v), ServiceState::Live);
}

#[tokio::test]
async fn patch_stores_settings_and_calls_the_engine() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "image": "nginx:alpine", "health_check_path": "/h"})).await;
    let id = v["id"].as_str().unwrap().to_string();
    app.engine.clear();

    // plain settings: stored, no engine calls
    let v = app
        .patch(
            "/api/v1/services/web",
            json!({"start_command": "nginx -g 'daemon off;'", "health_check_path": "", "port": 8080, "auto_deploy": false}),
        )
        .await
        .json();
    assert_eq!(v["start_command"], "nginx -g 'daemon off;'");
    assert_eq!(v["health_check_path"], Value::Null);
    assert_eq!(v["port"], 8080);
    assert_eq!(v["auto_deploy"], false);
    assert!(app.engine.calls().is_empty(), "{:?}", app.engine.calls());

    // port 0 clears
    let v = app.patch("/api/v1/services/web", json!({"port": 0})).await.json();
    assert_eq!(v["port"], Value::Null);

    // instances → scale, suspended → suspend, domains → refresh_routes
    let v = app
        .patch("/api/v1/services/web", json!({"instances": 3, "suspended": true, "custom_domains": ["x.example.com"]}))
        .await
        .json();
    assert_eq!(v["instances"], 3);
    assert_eq!(v["suspended"], true);
    assert_eq!(state_of(&v), ServiceState::Suspended);
    assert_eq!(
        app.engine.calls(),
        vec![format!("scale {id} 3"), format!("suspend {id}"), format!("refresh_routes {id}")]
    );
    app.engine.clear();

    // unchanged values don't call the engine again
    app.patch("/api/v1/services/web", json!({"instances": 3, "suspended": true, "custom_domains": ["x.example.com"]}))
        .await;
    assert!(app.engine.calls().is_empty(), "{:?}", app.engine.calls());

    let v = app.patch("/api/v1/services/web", json!({"suspended": false})).await.json();
    assert_eq!(v["suspended"], false);
    assert_eq!(app.engine.calls(), vec![format!("resume {id}")]);
}

#[tokio::test]
async fn patch_switches_sources_and_validates() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web", "image": "nginx:alpine"})).await;
    // switching to git drops the image and the image runtime
    let v = app.patch("/api/v1/services/web", json!({"repo_url": "https://github.com/a/b"})).await.json();
    assert_eq!(v["repo_url"], "https://github.com/a/b");
    assert_eq!(v["image"], Value::Null);
    assert_eq!(v["runtime"], "auto");
    // and back
    let v = app.patch("/api/v1/services/web", json!({"image": "busybox"})).await.json();
    assert_eq!(v["repo_url"], Value::Null);
    assert_eq!(v["runtime"], "image");

    for body in [
        json!({"instances": 0}),
        json!({"schedule": "* * * * *"}),
        json!({"custom_domains": ["bad domain"]}),
        json!({"repo_url": "https://a/b", "image": "x"}),
        json!({"disk_mount_path": "relative"}),
    ] {
        let r = app.patch("/api/v1/services/web", body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
    }
    app.create_service(json!({"name": "other", "custom_domains": ["taken.example.com"]})).await;
    let r = app.patch("/api/v1/services/web", json!({"custom_domains": ["taken.example.com"]})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = app.patch("/api/v1/services/nope", json!({})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_does_not_clobber_the_live_deploy() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web"})).await;
    let live = app.make_live("web", None).await;
    let v = app.patch("/api/v1/services/web", json!({"build_command": "make"})).await.json();
    assert_eq!(v["live_deploy_id"], live.id.as_str());
}

#[tokio::test]
async fn delete_goes_through_the_engine() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web"})).await;
    let r = app.delete("/api/v1/services/web").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(app.engine.calls(), vec![format!("delete_service {}", v["id"].as_str().unwrap())]);
    assert_eq!(app.get("/api/v1/services/web").await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.delete("/api/v1/services/web").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lifecycle_actions() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "image": "nginx:alpine"})).await;
    let id = v["id"].as_str().unwrap().to_string();
    let live = app.make_live("web", Some(80)).await;

    let r = app.post("/api/v1/services/web/restart", json!({})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(r.json()["trigger"], "restart");

    let r = app.post("/api/v1/services/web/scale", json!({"instances": 4})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["instances"], 4);
    assert_eq!(app.post("/api/v1/services/web/scale", json!({"instances": 0})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(app.post("/api/v1/services/web/scale", json!({})).await.status, StatusCode::BAD_REQUEST);

    let r = app.post("/api/v1/services/web/suspend", json!({})).await;
    assert_eq!(state_of(&r.json()), ServiceState::Suspended);
    // deploying a suspended service is a conflict (engine)
    assert_eq!(app.post("/api/v1/services/web/deploys", json!({})).await.status, StatusCode::CONFLICT);
    let r = app.post("/api/v1/services/web/resume", json!({})).await;
    assert_eq!(r.json()["suspended"], false);

    let r = app.post("/api/v1/services/web/rollback", json!({"deploy_id": live.id})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(r.json()["trigger"], "rollback");
    assert_eq!(
        app.post("/api/v1/services/web/rollback", json!({"deploy_id": "dep-nope"})).await.status,
        StatusCode::NOT_FOUND
    );
    app.create_service(json!({"name": "other"})).await;
    let foreign = app.make_live("other", None).await;
    let r = app.post("/api/v1/services/web/rollback", json!({"deploy_id": foreign.id})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let r = app.get("/api/v1/services/web/status").await;
    assert_eq!(r.status, StatusCode::OK);
    let s = r.json();
    assert_eq!(s["service_id"], id.as_str());
    assert_eq!(s["desired_instances"], 4);
    assert_eq!(s["instances"][0]["state"], "running");
}

#[tokio::test]
async fn deploys_trigger_list_get_cancel() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "repo_url": "https://github.com/a/b"})).await;
    let id = v["id"].as_str().unwrap().to_string();
    app.engine.clear();
    let r = app.post("/api/v1/services/web/deploys", json!({"commit": "abc123", "clear_cache": true})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let d = r.json();
    assert_eq!(d["trigger"], "manual");
    assert_eq!(d["commit_sha"], "abc123");
    assert_eq!(app.engine.calls(), vec![format!("deploy {id} manual commit=abc123 clear_cache=true source=-")]);
    // bare POST without a body works too
    let r = app.call(Method::POST, "/api/v1/services/web/deploys", None).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
    for bad in ["a b", "--upload-pack=x", "x\ny"] {
        let r = app.post("/api/v1/services/web/deploys", json!({ "commit": bad })).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }

    let list = app.get("/api/v1/services/web/deploys?limit=2").await.json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    let all = app.get("/api/v1/services/web/deploys").await.json();
    assert_eq!(all.as_array().unwrap().len(), 3);

    let dep_id = d["id"].as_str().unwrap();
    assert_eq!(app.get(&format!("/api/v1/deploys/{dep_id}")).await.json()["id"], dep_id);
    assert_eq!(app.get("/api/v1/deploys/dep-missing").await.status, StatusCode::NOT_FOUND);
    let r = app.post(&format!("/api/v1/deploys/{dep_id}/cancel"), json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["status"], "canceled");
    assert_eq!(app.post(&format!("/api/v1/deploys/{dep_id}/cancel"), json!({})).await.status, StatusCode::CONFLICT);
    assert_eq!(app.post("/api/v1/services/nope/deploys", json!({})).await.status, StatusCode::NOT_FOUND);
}

fn tar_gz() -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let data = b"print('hi')\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, "app.py", &data[..]).unwrap();
    let mut gz = builder.into_inner().unwrap();
    gz.flush().unwrap();
    gz.finish().unwrap()
}

fn upload_req(uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/gzip")
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn upload_gzip_queues_an_archive_deploy() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "app"})).await;
    let id = v["id"].as_str().unwrap();
    let archive = tar_gz();
    let r = app.send(upload_req("/api/v1/services/app/deploys/upload?clear_cache=true", archive.clone())).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
    let d = r.json();
    assert_eq!(d["trigger"], "upload");
    assert_eq!(d["source"]["kind"], "archive");
    let path = d["source"]["path"].as_str().unwrap();
    assert!(path.starts_with(app.config.uploads_dir().to_str().unwrap()), "{path}");
    assert!(path.ends_with(".tar.gz"));
    assert_eq!(std::fs::read(path).unwrap(), archive);
    let calls = app.engine.calls_with("deploy ");
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with(&format!("deploy {id} upload commit=- clear_cache=true source=")), "{}", calls[0]);
}

#[tokio::test]
async fn upload_rejects_non_gzip_and_cleans_up() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "app"})).await;
    let r = app.send(upload_req("/api/v1/services/app/deploys/upload", b"PK\x03\x04 zip file".to_vec())).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.code(), "invalid_request");
    let r = app.send(upload_req("/api/v1/services/app/deploys/upload", Vec::new())).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(app.engine.calls_with("deploy ").is_empty());
    // the partial files are removed (asynchronously)
    let dir = app.config.uploads_dir();
    for _ in 0..100 {
        if std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    // unknown service → 404 before reading the body
    let r = app.send(upload_req("/api/v1/services/nope/deploys/upload", tar_gz())).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_larger_than_the_json_limit_is_accepted() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "app"})).await;
    // 3 MiB: above axum's default 2 MiB body limit, below the upload limit.
    let mut body = vec![0x1f, 0x8b];
    body.extend(std::iter::repeat_n(7u8, 3 * 1024 * 1024));
    let r = app.send(upload_req("/api/v1/services/app/deploys/upload", body)).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
}

#[tokio::test]
async fn upload_with_oversized_content_length_is_rejected() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "app"})).await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/services/app/deploys/upload")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-length", (ferry_api::UPLOAD_LIMIT as u64 + 1).to_string())
        .body(Body::from(vec![0x1f, 0x8b]))
        .unwrap();
    let r = app.send(req).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.code(), "payload_too_large");
}

/// Split an SSE body into (event, data) pairs.
fn sse_events(text: &str) -> Vec<(String, String)> {
    text.split("\n\n")
        .filter(|e| !e.trim().is_empty() && !e.starts_with(':'))
        .map(|e| {
            let mut event = String::new();
            let mut data = String::new();
            for line in e.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    event = v.to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push_str(v.strip_prefix(' ').unwrap_or(v));
                }
            }
            (event, data)
        })
        .collect()
}

#[tokio::test]
async fn log_streams_are_sse() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "image": "nginx"})).await;
    let dep = v["latest_deploy"]["id"].as_str().unwrap().to_string();

    // runtime logs, finite (follow=false), via ?access_token= like EventSource
    let r = app
        .send(
            Request::builder()
                .uri(format!("/api/v1/services/web/logs?tail=10&access_token={TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["content-type"], "text/event-stream");
    assert_eq!(r.headers["cache-control"], "no-cache");
    let events = sse_events(&r.text());
    assert_eq!(events.len(), 3, "{}", r.text());
    assert_eq!(events[0].0, "log");
    let line: ferry_core::LogLine = serde_json::from_str(&events[0].1).unwrap();
    assert_eq!(line.line, "==> runtime one");
    assert_eq!(line.instance.as_deref(), Some("abc123"));
    assert_eq!(events[2], ("end".to_string(), String::new()));
    assert!(r.text().ends_with("event: end\ndata: \n\n"));
    assert!(
        app.engine.calls().contains(&format!("service_logs {} follow=false tail=Some(10)", v["id"].as_str().unwrap()))
    );

    // deploy logs are always finite
    let r = app.get(&format!("/api/v1/deploys/{dep}/logs?follow=true")).await;
    let events = sse_events(&r.text());
    assert_eq!(events.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(), ["log", "log", "end"]);
    assert!(app.engine.calls().contains(&format!("deploy_logs {dep} follow=true")));
    assert_eq!(app.get("/api/v1/deploys/dep-nope/logs").await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/api/v1/services/nope/logs").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn follow_runtime_logs_stream_without_end() {
    use futures::StreamExt;
    use tower::ServiceExt;
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web"})).await;
    let req = Request::builder()
        .uri("/api/v1/services/web/logs?follow=1")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body().into_data_stream();
    let mut seen = String::new();
    while seen.matches("event: log").count() < 2 {
        let chunk = body.next().await.unwrap().unwrap();
        seen.push_str(std::str::from_utf8(&chunk).unwrap());
    }
    // the stream stays open (no end event) — the next poll would block
    let next = tokio::time::timeout(std::time::Duration::from_millis(50), body.next()).await;
    assert!(next.is_err(), "stream should still be open");
    assert!(!seen.contains("event: end"));
}

#[tokio::test]
async fn deploy_status_values_roundtrip() {
    // Guards the JSON contract the CLI/dashboard rely on.
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web", "image": "nginx"})).await;
    let d = &app.get("/api/v1/services/web/deploys").await.json()[0];
    let status: DeployStatus = serde_json::from_value(d["status"].clone()).unwrap();
    assert_eq!(status, DeployStatus::Queued);
}
