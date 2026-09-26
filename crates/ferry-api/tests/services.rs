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
async fn internal_port_is_the_live_deploys_port() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "backend", "type": "pserv"})).await;
    assert_eq!(app.get("/api/v1/services/backend").await.json()["internal_port"], Value::Null);
    app.make_live("backend", Some(3000)).await;
    let v = app.get("/api/v1/services/backend").await.json();
    assert_eq!(v["internal_port"], 3000);
    assert_eq!(state_of(&v), ServiceState::Live);

    // A changed port setting only applies from the next deploy: the view
    // keeps the port the live instances listen on until then.
    let r = app.patch("/api/v1/services/backend", json!({"port": 4000})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["port"], 4000);
    assert_eq!(r.json()["internal_port"], 3000);
    app.make_live("backend", Some(4000)).await;
    assert_eq!(app.get("/api/v1/services/backend").await.json()["internal_port"], 4000);
    // a live deploy without a recorded port falls back to the setting
    app.make_live("backend", None).await;
    assert_eq!(app.get("/api/v1/services/backend").await.json()["internal_port"], 4000);
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

    // instances → scale, suspended → suspend (first: no containers are
    // started only to be stopped), domains → refresh_routes
    let v = app
        .patch("/api/v1/services/web", json!({"instances": 3, "suspended": true, "custom_domains": ["x.example.com"]}))
        .await
        .json();
    assert_eq!(v["instances"], 3);
    assert_eq!(v["suspended"], true);
    assert_eq!(state_of(&v), ServiceState::Suspended);
    assert_eq!(
        app.engine.calls(),
        vec![format!("suspend {id}"), format!("scale {id} 3"), format!("refresh_routes {id}")]
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
    // replacing the source is never a silent side effect: the old one must be
    // cleared in the same request (409 naming both otherwise)
    let r = app.patch("/api/v1/services/web", json!({"repo_url": "https://github.com/a/b"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(msg.contains("nginx:alpine") && msg.contains("--image \"\""), "{msg}");
    assert_eq!(app.store.require_service("web").await.unwrap().image.as_deref(), Some("nginx:alpine"));
    // switching to git drops the image and the image runtime
    let v = app.patch("/api/v1/services/web", json!({"repo_url": "https://github.com/a/b", "image": ""})).await.json();
    assert_eq!(v["repo_url"], "https://github.com/a/b");
    assert_eq!(v["image"], Value::Null);
    assert_eq!(v["runtime"], "auto");
    // and back (credentials in the repo URL are never echoed)
    app.patch("/api/v1/services/web", json!({"repo_url": "https://u:tok3n@github.com/a/b", "image": ""})).await;
    let r = app.patch("/api/v1/services/web", json!({"image": "busybox"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert!(!r.text().contains("tok3n") && r.text().contains("--repo \\\"\\\""), "{}", r.text());
    let v = app.patch("/api/v1/services/web", json!({"image": "busybox", "repo_url": ""})).await.json();
    assert_eq!(v["repo_url"], Value::Null);
    assert_eq!(v["runtime"], "image");
    // re-setting the same kind of source is not a switch
    let v = app.patch("/api/v1/services/web", json!({"image": "busybox:stable"})).await.json();
    assert_eq!(v["image"], "busybox:stable");

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
    let id = v["id"].as_str().unwrap();
    // A public service created without a source gets its routes right away.
    assert_eq!(app.engine.calls(), vec![format!("refresh_routes {id}"), format!("delete_service {id}")]);
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

// ---------------------------------------------------------------------------
// regressions

#[tokio::test]
async fn source_paths_outside_the_checkout_are_rejected_up_front() {
    let app = TestApp::new().await;
    let cases = [
        json!({"name": "trav-root", "repo_url": "/srv/git/mono", "root_dir": "../../"}),
        json!({"name": "trav-df", "repo_url": "/srv/git/mono", "dockerfile_path": "../../../../../etc/hosts"}),
        json!({"name": "trav-df2", "repo_url": "/srv/git/mono", "root_dir": "api", "dockerfile_path": "../../Dockerfile"}),
        json!({"name": "trav-pub", "type": "static", "publish_dir": "/etc"}),
    ];
    for body in cases {
        let r = app.post("/api/v1/services", body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
    }
    let r = app
        .post("/api/v1/services", json!({"name": "x", "repo_url": "/srv/git/mono", "dockerfile_path": "../Dockerfile"}))
        .await;
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("outside the source directory"), "{}", r.text());
    // a Dockerfile next to the root directory is fine
    app.create_service(
        json!({"name": "ok", "repo_url": "/srv/git/mono", "root_dir": "api", "dockerfile_path": "../Dockerfile", "deploy": false}),
    )
    .await;
    // and PATCH checks the combination too
    let r = app.patch("/api/v1/services/ok", json!({"root_dir": ""})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    let r = app.patch("/api/v1/services/ok", json!({"root_dir": "../x"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
}

#[tokio::test]
async fn invalid_git_inputs_are_rejected_by_the_api() {
    let app = TestApp::new().await;
    for body in [
        json!({"name": "w1", "repo_url": "-oProxyCommand=x"}),
        json!({"name": "w2", "repo_url": "/tmp", "branch": "--upload-pack=touch"}),
        json!({"name": "w3", "repo_url": "relative/path"}),
    ] {
        let r = app.post("/api/v1/services", body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
    }
    app.create_service(json!({"name": "probe", "repo_url": "https://github.com/a/b", "deploy": false})).await;
    for bad in ["zzzz", "--help", "abc", "a b"] {
        let r = app.post("/api/v1/services/probe/deploys", json!({ "commit": bad })).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let r = app.patch("/api/v1/services/probe", json!({"branch": "-evil"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // a commit only makes sense for git services
    app.create_service(json!({"name": "img", "image": "nginx", "deploy": false})).await;
    let r = app.post("/api/v1/services/img/deploys", json!({"commit": "abc123"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(app.engine.calls_with("deploy ").is_empty());
}

#[tokio::test]
async fn uploads_are_refused_for_services_with_a_repo_or_image() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "ws", "image": "jmalloc/echo-server", "port": 8080, "deploy": false})).await;
    app.create_service(json!({"name": "gs", "repo_url": "https://user:tok3n@github.com/a/b", "deploy": false})).await;
    for svc in ["ws", "gs"] {
        let r = app.send(upload_req(&format!("/api/v1/services/{svc}/deploys/upload"), tar_gz())).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{svc}: {}", r.text());
        assert!(!r.text().contains("tok3n"), "{}", r.text());
    }
    assert!(app.engine.calls_with("deploy ").is_empty());
    assert_eq!(std::fs::read_dir(app.config.uploads_dir()).map(|d| d.count()).unwrap_or(0), 0);
    // after clearing the source, uploads work
    app.patch("/api/v1/services/ws", json!({"image": ""})).await;
    let r = app.send(upload_req("/api/v1/services/ws/deploys/upload", tar_gz())).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
}

#[tokio::test]
async fn cron_jobs_have_no_instances_and_logs_point_at_job_runs() {
    let app = TestApp::new().await;
    let cron = json!({"name": "tick", "type": "cron", "schedule": "* * * * *", "image": "busybox", "deploy": false});
    let mut with_instances = cron.clone();
    with_instances["instances"] = json!(3);
    assert_eq!(app.post("/api/v1/services", with_instances).await.status, StatusCode::BAD_REQUEST);
    app.create_service(cron).await;
    let r = app.post("/api/v1/services/tick/scale", json!({"instances": 3})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("cron"), "{}", r.text());
    assert_eq!(app.patch("/api/v1/services/tick", json!({"instances": 2})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(app.patch("/api/v1/services/tick", json!({"instances": 1})).await.status, StatusCode::OK);
    assert!(app.engine.calls_with("scale").is_empty());

    // runtime logs: a finite hint (even with follow), never an endless empty stream
    app.post("/api/v1/services/tick/jobs", json!({})).await;
    let r = app.get("/api/v1/services/tick/logs?follow=1").await;
    assert_eq!(r.status, StatusCode::OK);
    let events = sse_events(&r.text());
    assert_eq!(events.last().unwrap().0, "end", "{}", r.text());
    assert!(r.text().contains("ferry jobs tick") && r.text().contains("Latest run: job-"), "{}", r.text());
    assert!(app.engine.calls_with("service_logs").is_empty());
}

#[tokio::test]
async fn a_live_cron_jobs_new_command_applies_through_a_restart() {
    let app = TestApp::new().await;
    let v = app
        .create_service(json!({"name": "tick", "type": "cron", "schedule": "* * * * *", "image": "busybox", "start_command": "echo one", "deploy": false}))
        .await;
    let id = v["id"].as_str().unwrap().to_string();
    // not deployed yet: the next deploy picks the command up
    let r = app.patch("/api/v1/services/tick", json!({"start_command": "echo two"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(app.engine.calls_with("restart").is_empty());

    app.make_live("tick", None).await;
    app.engine.clear();
    // runs use the live deploy's snapshot: a changed command needs a restart
    let r = app.patch("/api/v1/services/tick", json!({"start_command": "echo three"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["start_command"], "echo three");
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {id} restart")]);
    // same command, other settings: no restart
    app.engine.clear();
    app.patch("/api/v1/services/tick", json!({"start_command": "echo three", "schedule": "*/5 * * * *"})).await;
    assert!(app.engine.calls_with("restart").is_empty());
    // a failed restart is reported, the setting is saved
    app.engine.fail_restart.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = app.patch("/api/v1/services/tick", json!({"start_command": "echo four"})).await;
    assert!(r.status.is_server_error(), "{}", r.text());
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("settings saved"), "{}", r.text());
    assert_eq!(app.store.require_service("tick").await.unwrap().start_command.as_deref(), Some("echo four"));
    app.engine.fail_restart.store(false, std::sync::atomic::Ordering::SeqCst);
    // suspended: nothing to restart
    app.store.set_suspended(&id, true).await.unwrap();
    app.engine.clear();
    assert_eq!(app.patch("/api/v1/services/tick", json!({"start_command": "echo five"})).await.status, StatusCode::OK);
    assert!(app.engine.calls_with("restart").is_empty());

    // other service types: the command applies with the next deploy
    app.create_service(json!({"name": "web", "image": "nginx", "deploy": false})).await;
    app.make_live("web", Some(80)).await;
    app.engine.clear();
    app.patch("/api/v1/services/web", json!({"start_command": "nginx -g 'daemon off;'"})).await;
    assert!(app.engine.calls_with("restart").is_empty());
}

#[tokio::test]
async fn patch_resumes_after_scaling_and_reports_partial_failures() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "image": "nginx"})).await;
    let id = v["id"].as_str().unwrap().to_string();
    app.post("/api/v1/services/web/suspend", json!({})).await;
    app.engine.clear();
    app.patch("/api/v1/services/web", json!({"instances": 2, "suspended": false})).await;
    assert_eq!(app.engine.calls(), vec![format!("scale {id} 2"), format!("resume {id}")]);

    // a failed side effect says what was already saved
    app.engine.fail_scale.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = app.patch("/api/v1/services/web", json!({"build_command": "make", "instances": 3})).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(msg.starts_with("settings saved, but scaling 'web' to 3 failed"), "{msg}");
    assert_eq!(app.store.require_service("web").await.unwrap().build_command.as_deref(), Some("make"));
    // nothing saved before the failure: the plain error
    let r = app.patch("/api/v1/services/web", json!({"instances": 4})).await;
    assert!(!r.json()["error"]["message"].as_str().unwrap().contains("saved"), "{}", r.text());
}

#[tokio::test]
async fn deploy_hook_key_can_be_rotated() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "image": "nginx", "deploy": false})).await;
    let old_path = v["deploy_hook_path"].as_str().unwrap().to_string();
    let r = app.post("/api/v1/services/web/deploy-hook/rotate", json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let new_path = r.json()["deploy_hook_path"].as_str().unwrap().to_string();
    assert_ne!(old_path, new_path);
    let hook = |p: &str| Request::builder().method(Method::POST).uri(p).body(Body::empty()).unwrap();
    assert_eq!(app.send(hook(&old_path)).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.send(hook(&new_path)).await.status, StatusCode::ACCEPTED);
    assert_eq!(app.post("/api/v1/services/nope/deploy-hook/rotate", json!({})).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_a_referenced_service_needs_force() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "echo2", "type": "pserv"})).await;
    app.create_service(
        json!({"name": "echo1", "env": [{"key": "API", "value": "http://${{ service.echo2.hostport }}"}]}),
    )
    .await;
    // a self-reference doesn't block
    app.create_service(json!({"name": "solo", "env": [{"key": "ME", "value": "${{svc.solo.host}}"}]})).await;
    let r = app.delete("/api/v1/services/echo2").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(msg.contains("service 'echo1' (API)") && msg.contains("force=true"), "{msg}");
    assert!(app.engine.calls_with("delete_service").is_empty());
    assert_eq!(app.delete("/api/v1/services/echo2?force=true").await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.delete("/api/v1/services/solo").await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn follow_streams_end_when_the_server_shuts_down() {
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
    let mut body = resp.into_body().into_data_stream();
    let mut seen = String::new();
    while seen.matches("event: log").count() < 2 {
        seen.push_str(std::str::from_utf8(&body.next().await.unwrap().unwrap()).unwrap());
    }
    app.shutdown.cancel();
    let next = tokio::time::timeout(std::time::Duration::from_secs(2), body.next()).await;
    assert!(matches!(next, Ok(None)), "the stream must end on shutdown");
}

#[tokio::test]
async fn unknown_fields_are_json_400s() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web", "image": "nginx", "deploy": false})).await;
    let cases = [
        (Method::POST, "/api/v1/services", json!({"name": "x", "imagee": "nginx"})),
        (Method::PATCH, "/api/v1/services/web", json!({"instance": 2})),
        (Method::POST, "/api/v1/services/web/scale", json!({"instances": 2, "force": true})),
        (Method::PATCH, "/api/v1/services/web/env", json!({"sett": []})),
        (Method::POST, "/api/v1/services/web/deploys", json!({"sha": "abc123"})),
        (Method::POST, "/api/v1/datastores", json!({"name": "d", "kind": "redis", "size": 1})),
        (Method::POST, "/api/v1/blueprints/apply", json!({"yaml": "", "dryrun": true})),
    ];
    for (method, uri, body) in cases {
        let r = app.call(method.clone(), uri, Some(body.clone())).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{method} {uri} {body}: {}", r.text());
        assert_eq!(r.code(), "invalid_request");
        assert!(r.json()["error"]["message"].as_str().unwrap().contains("unknown field"), "{}", r.text());
    }
    // Only the route installation from creating "web" (deploy: false).
    assert!(app.engine.calls().iter().all(|c| c.starts_with("refresh_routes")), "{:?}", app.engine.calls());
}

#[tokio::test]
async fn public_services_without_a_deploy_get_routes_immediately() {
    let app = TestApp::new().await;
    let web = app.create_service(json!({"name": "web", "deploy": false, "image": "nginx"})).await;
    let worker = app.create_service(json!({"name": "bg", "type": "worker", "deploy": false, "image": "busybox"})).await;
    let calls = app.engine.calls();
    assert!(calls.contains(&format!("refresh_routes {}", web["id"].as_str().unwrap())), "{calls:?}");
    assert!(!calls.iter().any(|c| c.contains(worker["id"].as_str().unwrap())), "{calls:?}");
}

#[tokio::test]
async fn views_report_degraded_when_live_instances_are_down() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "dsk", "deploy": false})).await;
    app.make_live("dsk", Some(8080)).await;
    *app.engine.running.lock().unwrap() = Some(0);
    let v = app.get("/api/v1/services/dsk").await.json();
    assert_eq!(state_of(&v), ServiceState::Degraded, "{v}");
    let list = app.get("/api/v1/services").await.json();
    assert_eq!(state_of(&list[0]), ServiceState::Degraded, "{list}");
    // counts are cached briefly: the list didn't ask the engine again
    assert_eq!(app.engine.calls_with("service_status").len(), 1);

    // every status call refreshes the count
    *app.engine.running.lock().unwrap() = None;
    assert_eq!(state_of(&app.get("/api/v1/services/dsk/status").await.json()), ServiceState::Live);
    assert_eq!(state_of(&app.get("/api/v1/services/dsk").await.json()), ServiceState::Live);

    // suspended or never deployed services don't need the engine at all
    app.create_service(json!({"name": "idle", "deploy": false})).await;
    app.post("/api/v1/services/dsk/suspend", json!({})).await;
    app.engine.clear();
    let list = app.get("/api/v1/services").await.json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(app.engine.calls_with("service_status").is_empty());
}
