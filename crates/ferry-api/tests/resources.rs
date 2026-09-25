//! Env vars, env groups, custom domains, jobs and datastores.

mod common;

use std::sync::atomic::Ordering;

use common::TestApp;
use http::{Method, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn service_env_put_and_patch() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web", "env": [{"key": "OLD", "value": "x"}]})).await;
    let id = v["id"].as_str().unwrap().to_string();

    let r = app
        .put("/api/v1/services/web/env", json!({"vars": [{"key": "B", "value": "2"}, {"key": "A", "value": "1"}]}))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!([{"key": "A", "value": "1"}, {"key": "B", "value": "2"}]));

    let r = app
        .patch(
            "/api/v1/services/web/env",
            json!({"set": [{"key": "C", "value": "3"}, {"key": "A", "value": "9"}], "unset": ["B", "NOPE"]}),
        )
        .await;
    assert_eq!(r.json(), json!([{"key": "A", "value": "9"}, {"key": "C", "value": "3"}]));

    // validation
    let r = app.put("/api/v1/services/web/env", json!({"vars": [{"key": "A=B", "value": "1"}]})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app.patch("/api/v1/services/web/env", json!({"set": [{"key": "", "value": "1"}]})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(app.get("/api/v1/services/web/env").await.json().as_array().unwrap().len(), 2);

    // restart=true on a service without a live deploy: nothing to restart
    let r = app.patch("/api/v1/services/web/env?restart=true", json!({"set": [{"key": "D", "value": "4"}]})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(app.engine.calls_with("restart").is_empty());

    // live service: restart with the env_change trigger
    app.make_live("web", Some(80)).await;
    let r = app.put("/api/v1/services/web/env?restart=true", json!({"vars": []})).await;
    assert_eq!(r.json(), json!([]));
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {id} env_change")]);
    // without restart
    app.engine.clear();
    app.patch("/api/v1/services/web/env", json!({"set": [{"key": "E", "value": "5"}]})).await;
    app.patch("/api/v1/services/web/env?restart=false", json!({"set": [{"key": "E", "value": "6"}]})).await;
    assert!(app.engine.calls_with("restart").is_empty());

    // a failed restart is reported, but the vars are saved
    app.engine.fail_restart.store(true, Ordering::SeqCst);
    let r = app.patch("/api/v1/services/web/env?restart=1", json!({"set": [{"key": "F", "value": "7"}]})).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("env vars saved"));
    assert!(app.store.list_env(&id).await.unwrap().iter().any(|v| v.key == "F"));

    // suspended services are not restarted
    app.engine.fail_restart.store(false, Ordering::SeqCst);
    app.engine.clear();
    app.store.set_suspended(&id, true).await.unwrap();
    app.patch("/api/v1/services/web/env?restart=true", json!({"set": [{"key": "G", "value": "8"}]})).await;
    assert!(app.engine.calls_with("restart").is_empty());

    assert_eq!(app.get("/api/v1/services/nope/env").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn env_groups_crud_link_unlink_restart() {
    let app = TestApp::new().await;
    let r = app.post("/api/v1/env-groups", json!({"name": "shared", "vars": [{"key": "X", "value": "1"}]})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let g = r.json();
    assert_eq!(g["name"], "shared");
    assert_eq!(g["vars"], json!([{"key": "X", "value": "1"}]));
    assert_eq!(g["services"], json!([]));
    let gid = g["id"].as_str().unwrap().to_string();

    assert_eq!(app.post("/api/v1/env-groups", json!({"name": "shared"})).await.status, StatusCode::CONFLICT);
    assert_eq!(app.post("/api/v1/env-groups", json!({"name": "bad name"})).await.status, StatusCode::BAD_REQUEST);
    let bad_var = json!({"name": "other", "vars": [{"key": "A B", "value": "1"}]});
    assert_eq!(app.post("/api/v1/env-groups", bad_var).await.status, StatusCode::BAD_REQUEST);
    assert!(app.store.find_env_group("other").await.unwrap().is_none());

    // link two services, one live
    let web = app.create_service(json!({"name": "web"})).await;
    let worker = app.create_service(json!({"name": "worker", "type": "worker"})).await;
    let r = app.post("/api/v1/services/web/env-groups", json!({"group": "shared"})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["env_groups"], json!(["shared"]));
    app.post("/api/v1/services/worker/env-groups", json!({"group": gid})).await;
    assert_eq!(
        app.post("/api/v1/services/web/env-groups", json!({"group": "nope"})).await.status,
        StatusCode::NOT_FOUND
    );
    app.make_live("web", Some(80)).await;

    let g = app.get("/api/v1/env-groups/shared").await.json();
    assert_eq!(g["services"], json!(["web", "worker"]));
    let list = app.get("/api/v1/env-groups").await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);

    // PUT with restart → only the live service restarts
    app.engine.clear();
    let r = app.put("/api/v1/env-groups/shared/env?restart=true", json!({"vars": [{"key": "Y", "value": "2"}]})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["vars"], json!([{"key": "Y", "value": "2"}]));
    assert_eq!(app.engine.calls(), vec![format!("restart {} env_change", web["id"].as_str().unwrap())]);

    // a failed restart doesn't fail the group request
    app.engine.fail_restart.store(true, Ordering::SeqCst);
    let r = app
        .patch(
            "/api/v1/env-groups/shared/env?restart=true",
            json!({"set": [{"key": "Z", "value": "3"}], "unset": ["Y"]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["vars"], json!([{"key": "Z", "value": "3"}]));
    app.engine.fail_restart.store(false, Ordering::SeqCst);

    // no restart flag → no restart
    app.engine.clear();
    app.patch("/api/v1/env-groups/shared/env", json!({"set": [{"key": "Z", "value": "4"}]})).await;
    assert!(app.engine.calls().is_empty());
    let r = app.patch("/api/v1/env-groups/shared/env", json!({"set": [{"key": "bad key", "value": "4"}]})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // effective env of the service includes the group
    let eff = app.store.effective_env(web["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(eff, vec![ferry_core::EnvVar::new("Z", "4")]);

    // unlink
    let r = app.delete("/api/v1/services/worker/env-groups/shared").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["env_groups"], json!([]));
    assert_eq!(app.delete("/api/v1/services/worker/env-groups/shared").await.status, StatusCode::NOT_FOUND);
    let _ = worker;

    // delete
    assert_eq!(app.delete("/api/v1/env-groups/shared").await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.get("/api/v1/env-groups/shared").await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/api/v1/services/web").await.json()["env_groups"], json!([]));
}

#[tokio::test]
async fn custom_domains() {
    let app = TestApp::new().await;
    let v = app.create_service(json!({"name": "web"})).await;
    let id = v["id"].as_str().unwrap().to_string();
    app.create_service(json!({"name": "other", "custom_domains": ["taken.example.com"]})).await;
    app.create_service(json!({"name": "bg", "type": "worker"})).await;
    app.engine.clear();

    assert_eq!(app.get("/api/v1/services/web/domains").await.json(), json!([]));
    let r = app.post("/api/v1/services/web/domains", json!({"domain": "WWW.Example.com."})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!(["www.example.com"]));
    assert_eq!(app.engine.calls(), vec![format!("refresh_routes {id}")]);
    let r = app.post("/api/v1/services/web/domains", json!({"domain": "api.example.com"})).await;
    assert_eq!(r.json(), json!(["www.example.com", "api.example.com"]));
    assert_eq!(
        app.get("/api/v1/services/web").await.json()["hosts"],
        json!(["web.localhost", "www.example.com", "api.example.com"])
    );

    // errors
    let cases = [
        ("web", "www.example.com", StatusCode::CONFLICT),   // already added
        ("web", "taken.example.com", StatusCode::CONFLICT), // another service
        ("web", "other.localhost", StatusCode::CONFLICT),   // another service's default host
        ("web", "web.localhost", StatusCode::CONFLICT),     // own default host
        ("web", "ferry.localhost", StatusCode::CONFLICT),   // dashboard
        ("web", "not a domain", StatusCode::BAD_REQUEST),
        ("web", "localhost", StatusCode::BAD_REQUEST),
        ("bg", "bg.example.com", StatusCode::BAD_REQUEST), // workers have no domains
        ("nope", "a.example.com", StatusCode::NOT_FOUND),
    ];
    for (svc, domain, status) in cases {
        let r = app.post(&format!("/api/v1/services/{svc}/domains"), json!({"domain": domain})).await;
        assert_eq!(r.status, status, "{svc} {domain}: {}", r.text());
    }

    app.engine.clear();
    let r = app.delete("/api/v1/services/web/domains/WWW.example.com").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!(["api.example.com"]));
    assert_eq!(app.engine.calls(), vec![format!("refresh_routes {id}")]);
    assert_eq!(app.delete("/api/v1/services/web/domains/www.example.com").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn jobs() {
    let app = TestApp::new().await;
    let cron = app
        .create_service(json!({"name": "nightly", "type": "cron", "schedule": "0 3 * * *", "image": "busybox", "start_command": "echo hi"}))
        .await;
    app.create_service(json!({"name": "web", "image": "nginx"})).await;
    let cron_id = cron["id"].as_str().unwrap().to_string();

    // cron jobs run their own command
    let r = app.call(Method::POST, "/api/v1/services/nightly/jobs", None).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
    let job = r.json();
    assert_eq!(job["trigger"], "manual");
    assert_eq!(job["status"], "pending");
    assert_eq!(app.engine.calls_with("run_job"), vec![format!("run_job {cron_id} - manual")]);

    // other services need a command
    let r = app.post("/api/v1/services/web/jobs", json!({"command": "  "})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app.post("/api/v1/services/web/jobs", json!({"command": "rake db:migrate"})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(r.json()["command"], "rake db:migrate");

    let list = app.get("/api/v1/services/nightly/jobs?limit=5").await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let job_id = job["id"].as_str().unwrap();
    assert_eq!(app.get(&format!("/api/v1/jobs/{job_id}")).await.json()["id"], job_id);
    assert_eq!(app.get("/api/v1/jobs/job-nope").await.status, StatusCode::NOT_FOUND);

    let r = app.get(&format!("/api/v1/jobs/{job_id}/logs?follow=true")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.text().contains("event: log\ndata: {"));
    assert!(r.text().ends_with("event: end\ndata: \n\n"));
    assert!(app.engine.calls().contains(&format!("job_logs {job_id} follow=true")));
    assert_eq!(app.get("/api/v1/jobs/job-nope/logs").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn datastores() {
    let app = TestApp::with_config(|c| c.advertise_host = "10.0.0.5".into()).await;
    let r = app.post("/api/v1/datastores", json!({"name": "main-db", "kind": "postgres"})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let v = r.json();
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["kind"], "postgres");
    assert_eq!(v["version"], "16");
    assert_eq!(v["database"], "main_db");
    assert_eq!(v["username"], "main_db");
    assert_eq!(v["status"], "creating");
    assert_eq!(v["internal_host"], "main-db");
    assert_eq!(v["internal_port"], 5432);
    let pw = v["password"].as_str().unwrap();
    assert_eq!(v["internal_url"], format!("postgresql://main_db:{pw}@main-db:5432/main_db"));
    assert_eq!(v["external_url"], format!("postgresql://main_db:{pw}@10.0.0.5:15432/main_db"));
    assert_eq!(app.engine.calls(), vec![format!("provision {id}")]);

    let r = app.post("/api/v1/datastores", json!({"name": "cache", "kind": "keyvalue", "version": "7.2"})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["kind"], "redis");
    assert_eq!(r.json()["version"], "7.2");
    assert_eq!(r.json()["internal_port"], 6379);

    let r = app.post("/api/v1/datastores", json!({"name": "custom", "kind": "pg", "database": "appdb"})).await;
    assert_eq!((r.json()["database"].clone(), r.json()["username"].clone()), (json!("appdb"), json!("appdb")));
    let r = app
        .post(
            "/api/v1/datastores",
            json!({"name": "custom2", "kind": "postgres", "database": "appdb", "username": "bob"}),
        )
        .await;
    assert_eq!(r.json()["username"], "bob");

    // validation & conflicts
    let cases = [
        (json!({"name": "Bad", "kind": "postgres"}), StatusCode::BAD_REQUEST),
        (json!({"name": "x", "kind": "mysql"}), StatusCode::BAD_REQUEST),
        (json!({"name": "x", "kind": "postgres", "version": "16; rm -rf"}), StatusCode::BAD_REQUEST),
        (json!({"name": "x", "kind": "postgres", "database": "bad-name"}), StatusCode::BAD_REQUEST),
        (json!({"name": "x", "kind": "redis", "username": "bob"}), StatusCode::BAD_REQUEST),
        (json!({"name": "main-db", "kind": "redis"}), StatusCode::CONFLICT),
    ];
    for (body, status) in cases {
        let r = app.post("/api/v1/datastores", body.clone()).await;
        assert_eq!(r.status, status, "{body}: {}", r.text());
    }
    app.create_service(json!({"name": "svc"})).await;
    assert_eq!(
        app.post("/api/v1/datastores", json!({"name": "svc", "kind": "redis"})).await.status,
        StatusCode::CONFLICT
    );

    // list / get / delete
    let list = app.get("/api/v1/datastores").await.json();
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["cache", "custom", "custom2", "main-db"]);
    assert_eq!(app.get("/api/v1/datastores/main-db").await.json()["id"], id.as_str());
    assert_eq!(app.get(&format!("/api/v1/datastores/{id}")).await.json()["name"], "main-db");
    app.engine.clear();
    assert_eq!(app.delete("/api/v1/datastores/main-db").await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.engine.calls(), vec![format!("delete_datastore {id}")]);
    assert_eq!(app.get("/api/v1/datastores/main-db").await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.delete("/api/v1/datastores/main-db").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn datastore_provisioning_failure_is_recorded() {
    let app = TestApp::new().await;
    app.engine.fail_provision.store(true, Ordering::SeqCst);
    let r = app.post("/api/v1/datastores", json!({"name": "db", "kind": "postgres"})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let v = r.json();
    assert_eq!(v["status"], "failed");
    assert!(v["error"].as_str().unwrap().contains("daemon unreachable"));
    assert_eq!(v["external_url"], Value::Null);
}
