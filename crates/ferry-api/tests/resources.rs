//! Env vars, env groups, custom domains, jobs and datastores.

mod common;

use std::sync::atomic::Ordering;

use common::TestApp;
use ferry_core::DatastoreStatus;
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
async fn env_writes_that_change_nothing_restart_nothing() {
    let app = TestApp::new().await;
    app.post("/api/v1/env-groups", json!({"name": "shared", "vars": [{"key": "X", "value": "1"}]})).await;
    let web = app
        .create_service(json!({"name": "web", "env": [{"key": "A", "value": "1"}, {"key": "B", "value": "2"}], "env_groups": ["shared"]}))
        .await;
    let api = app.create_service(json!({"name": "api", "env_groups": ["shared"]})).await;
    let (web_id, api_id) = (web["id"].as_str().unwrap().to_string(), api["id"].as_str().unwrap().to_string());
    app.make_live("web", Some(80)).await;
    app.make_live("api", Some(80)).await;
    app.engine.clear();

    // identical values (in another order), unsets of missing keys
    let same = json!({"vars": [{"key": "B", "value": "2"}, {"key": "A", "value": "1"}]});
    assert_eq!(app.put("/api/v1/services/web/env?restart=true", same).await.status, StatusCode::OK);
    let r = app
        .patch("/api/v1/services/web/env?restart=true", json!({"set": [{"key": "A", "value": "1"}], "unset": ["NOPE"]}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!([{"key": "A", "value": "1"}, {"key": "B", "value": "2"}]));
    // an own variable shadowing a group variable with the same value: stored,
    // but the effective environment is the same
    let r = app.patch("/api/v1/services/web/env?restart=true", json!({"set": [{"key": "X", "value": "1"}]})).await;
    assert_eq!(r.json().as_array().unwrap().len(), 3);
    assert!(app.engine.calls_with("restart").is_empty(), "{:?}", app.engine.calls());

    // env group: identical values → nothing written, nothing restarted
    let before = app.store.require_env_group("shared").await.unwrap().updated_at;
    let r = app.put("/api/v1/env-groups/shared/env?restart=true", json!({"vars": [{"key": "X", "value": "1"}]})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let r = app.patch("/api/v1/env-groups/shared/env?restart=true", json!({"unset": ["NOPE"]})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(app.store.require_env_group("shared").await.unwrap().updated_at, before);
    assert!(app.engine.calls_with("restart").is_empty(), "{:?}", app.engine.calls());

    // a group change only restarts the services that don't override it
    let r = app.put("/api/v1/env-groups/shared/env?restart=true", json!({"vars": [{"key": "X", "value": "2"}]})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {api_id} env_change")]);

    // a real change of the service's own vars restarts it
    app.engine.clear();
    app.patch("/api/v1/services/web/env?restart=true", json!({"set": [{"key": "A", "value": "9"}]})).await;
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {web_id} env_change")]);

    // deleting the group (forced, restart=true) only restarts services that lose variables
    app.engine.clear();
    let r = app.delete("/api/v1/env-groups/shared?force=true&restart=true").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {api_id} env_change")]);
}

#[tokio::test]
async fn unknown_fields_of_nested_env_vars_are_json_400s() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web"})).await;
    app.post("/api/v1/env-groups", json!({"name": "shared"})).await;
    let extra = json!({"key": "A", "value": "1", "secret": true});
    let cases = [
        (Method::PUT, "/api/v1/services/web/env", json!({"vars": [extra.clone()]})),
        (Method::PATCH, "/api/v1/services/web/env", json!({"set": [extra.clone()]})),
        (Method::PUT, "/api/v1/env-groups/shared/env", json!({"vars": [extra.clone()]})),
        (Method::PATCH, "/api/v1/env-groups/shared/env", json!({"set": [extra.clone()]})),
        (Method::POST, "/api/v1/env-groups", json!({"name": "g2", "vars": [extra.clone()]})),
        (Method::POST, "/api/v1/services", json!({"name": "svc2", "env": [extra.clone()]})),
    ];
    for (method, uri, body) in cases {
        let r = app.call(method.clone(), uri, Some(body)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{method} {uri}: {}", r.text());
        assert_eq!(r.code(), "invalid_request", "{method} {uri}");
        let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
        assert!(msg.contains("unknown field") && msg.contains("secret"), "{method} {uri}: {msg}");
    }
    assert!(app.store.list_env(&app.store.require_service("web").await.unwrap().id).await.unwrap().is_empty());
    assert!(app.store.find_env_group("g2").await.unwrap().is_none());
    assert!(app.store.find_service("svc2").await.unwrap().is_none());
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

    // delete: refused while linked (409 naming the services), unless forced
    let r = app.delete("/api/v1/env-groups/shared").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("'web'"), "{}", r.text());
    assert!(app.store.find_env_group("shared").await.unwrap().is_some());
    app.engine.clear();
    assert_eq!(app.delete("/api/v1/env-groups/shared?force=true").await.status, StatusCode::NO_CONTENT);
    assert!(app.engine.calls_with("restart").is_empty());
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

    // cancel: a pending job → canceled; a finished one → 409
    let r = app.post(&format!("/api/v1/jobs/{job_id}/cancel"), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["status"], "canceled");
    assert_eq!(r.json()["id"], job_id);
    assert!(app.engine.calls().contains(&format!("cancel_job {job_id}")));
    app.engine.clear();
    let r = app.call(Method::POST, &format!("/api/v1/jobs/{job_id}/cancel"), None).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert_eq!(r.code(), "conflict");
    assert!(app.engine.calls_with("cancel_job").is_empty());
    assert_eq!(app.post("/api/v1/jobs/job-nope/cancel", json!({})).await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get(&format!("/api/v1/jobs/{job_id}/cancel")).await.status, StatusCode::METHOD_NOT_ALLOWED);
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

#[tokio::test]
async fn datastore_limits_on_create() {
    let app = TestApp::new().await;
    let r = app.post("/api/v1/datastores", json!({"name": "db", "kind": "postgres"})).await;
    assert_eq!((r.json()["memory_limit_mb"].clone(), r.json()["cpu_limit"].clone()), (Value::Null, Value::Null));
    let r = app
        .post(
            "/api/v1/datastores",
            json!({"name": "big", "kind": "postgres", "memory_limit_mb": 2048, "cpu_limit": 1.234}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!((r.json()["memory_limit_mb"].clone(), r.json()["cpu_limit"].clone()), (json!(2048), json!(1.23)));
    let ds = app.store.require_datastore("big").await.unwrap();
    assert_eq!((ds.memory_limit_mb, ds.cpu_limit), (Some(2048), Some(1.23)));
    // 0 = the server default, as in PATCH
    let r = app
        .post("/api/v1/datastores", json!({"name": "kv", "kind": "redis", "memory_limit_mb": 0, "cpu_limit": 0}))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!((r.json()["memory_limit_mb"].clone(), r.json()["cpu_limit"].clone()), (Value::Null, Value::Null));

    // invalid limits: nothing is created or provisioned
    app.engine.clear();
    for body in [
        json!({"name": "x", "kind": "redis", "memory_limit_mb": 4}),
        json!({"name": "x", "kind": "redis", "cpu_limit": 600}),
        json!({"name": "x", "kind": "redis", "cpu_limit": -1}),
    ] {
        let r = app.post("/api/v1/datastores", body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
        assert!(r.json()["error"]["message"].as_str().unwrap().contains("limit must be between"), "{}", r.text());
    }
    assert!(app.store.find_datastore("x").await.unwrap().is_none());
    assert!(app.engine.calls().is_empty(), "{:?}", app.engine.calls());
}

#[tokio::test]
async fn datastore_limits_are_changed_in_place() {
    let app = TestApp::new().await;
    let v = app.post("/api/v1/datastores", json!({"name": "db", "kind": "postgres"})).await.json();
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["status"], "creating");
    app.engine.clear();

    // still creating: applied too (its container may exist already; the
    // engine does nothing when it doesn't)
    let r = app.patch("/api/v1/datastores/db", json!({"memory_limit_mb": 1024})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!((r.json()["memory_limit_mb"].clone(), r.json()["cpu_limit"].clone()), (json!(1024), Value::Null));
    assert_eq!(r.json()["name"], "db");
    assert!(r.json()["internal_url"].as_str().unwrap().starts_with("postgresql://"));
    assert_eq!(app.engine.calls(), vec![format!("update_limits {id} 1024 -")]);
    app.engine.clear();

    // available: applied to the container, by id or name, one field at a time
    let mut ds = app.store.require_datastore(&id).await.unwrap();
    ds.status = DatastoreStatus::Available;
    app.store.update_datastore(&ds).await.unwrap();
    let r = app.patch(&format!("/api/v1/datastores/{id}"), json!({"cpu_limit": 0.5})).await;
    assert_eq!((r.json()["memory_limit_mb"].clone(), r.json()["cpu_limit"].clone()), (json!(1024), json!(0.5)));
    assert_eq!(app.engine.calls(), vec![format!("update_limits {id} 1024 0.5")]);
    app.engine.clear();
    let r = app.patch("/api/v1/datastores/db", json!({"memory_limit_mb": 0, "cpu_limit": 2.346})).await;
    assert_eq!((r.json()["memory_limit_mb"].clone(), r.json()["cpu_limit"].clone()), (Value::Null, json!(2.35)));
    assert_eq!(app.engine.calls(), vec![format!("update_limits {id} - 2.35")]);
    // unchanged limits are applied again (retries a failed update)
    app.engine.clear();
    let r = app.patch("/api/v1/datastores/db", json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(app.engine.calls(), vec![format!("update_limits {id} - 2.35")]);

    // invalid values change nothing
    app.engine.clear();
    for body in
        [json!({"memory_limit_mb": 10}), json!({"cpu_limit": 512.5}), json!({"memory_limit_mb": 64, "cpu_limit": -2})]
    {
        let r = app.patch("/api/v1/datastores/db", body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
        assert!(r.json()["error"]["message"].as_str().unwrap().contains("limit must be between"), "{}", r.text());
    }
    let r = app.patch("/api/v1/datastores/db", json!({"version": "17"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    let ds = app.store.require_datastore(&id).await.unwrap();
    assert_eq!((ds.memory_limit_mb, ds.cpu_limit, ds.version.as_str()), (None, Some(2.35), "16"));
    assert!(app.engine.calls().is_empty(), "{:?}", app.engine.calls());
    assert_eq!(app.patch("/api/v1/datastores/nope", json!({"cpu_limit": 1})).await.status, StatusCode::NOT_FOUND);

    // the engine fails: the limits are saved, and the error says so
    app.engine.fail_update_limits.store(true, Ordering::SeqCst);
    let r = app.patch("/api/v1/datastores/db", json!({"memory_limit_mb": 256})).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY, "{}", r.text());
    let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(msg.starts_with("limits of 'db' saved, but applying them to its container failed: "), "{msg}");
    assert!(msg.contains("docker update exploded"), "{msg}");
    assert_eq!(app.store.require_datastore(&id).await.unwrap().memory_limit_mb, Some(256));

    // failed (e.g. out of memory): its container may still be there,
    // restarting with the old limits, and the next provisioning attempt
    // keeps it, so the new limits are applied to it too
    app.engine.fail_update_limits.store(false, Ordering::SeqCst);
    let mut ds = app.store.require_datastore(&id).await.unwrap();
    ds.status = DatastoreStatus::Failed;
    app.store.update_datastore(&ds).await.unwrap();
    app.engine.clear();
    let r = app.patch("/api/v1/datastores/db", json!({"memory_limit_mb": 512})).await;
    assert_eq!((r.status, r.json()["memory_limit_mb"].clone()), (StatusCode::OK, json!(512)));
    assert_eq!(app.engine.calls(), vec![format!("update_limits {id} 512 2.35")]);
}

// ---------------------------------------------------------------------------
// regressions

fn big(n: usize) -> String {
    "x".repeat(n)
}

#[tokio::test]
async fn env_sizes_are_limited_per_value_per_owner_and_combined() {
    use ferry_core::validate::{MAX_ENV_TOTAL_BYTES, MAX_ENV_VALUE_BYTES};
    let app = TestApp::new().await;
    app.create_service(json!({"name": "probe"})).await;

    // one oversized value (the original 1.5 MB report), named in the error
    let r = app.patch("/api/v1/services/probe/env", json!({"set": [{"key": "HUGE", "value": big(1_500_000)}]})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("HUGE"), "{}", &r.text()[..200]);
    let r = app
        .put("/api/v1/services/probe/env", json!({"vars": [{"key": "V", "value": big(MAX_ENV_VALUE_BYTES + 1)}]}))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app
        .post("/api/v1/services", json!({"name": "x", "env": [{"key": "V", "value": big(MAX_ENV_VALUE_BYTES + 1)}]}))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // duplicate keys in one request
    let r = app
        .put("/api/v1/services/probe/env", json!({"vars": [{"key": "A", "value": "1"}, {"key": "A", "value": "2"}]}))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // the total is checked on the merged result of PATCHes
    let chunk = big(MAX_ENV_VALUE_BYTES - 64);
    let per_patch = 3;
    let mut accepted = 0;
    for i in 0..20 {
        let set: Vec<Value> =
            (0..per_patch).map(|j| json!({"key": format!("K{i}_{j}"), "value": chunk.clone()})).collect();
        let r = app.patch("/api/v1/services/probe/env", json!({ "set": set })).await;
        if r.status == StatusCode::BAD_REQUEST {
            assert!(r.json()["error"]["message"].as_str().unwrap().contains("maximum"), "{}", r.text());
            break;
        }
        assert_eq!(r.status, StatusCode::OK);
        accepted += 1;
    }
    assert!(accepted > 0 && accepted < 20);
    let id = app.store.require_service("probe").await.unwrap().id;
    let total: usize = app.store.list_env(&id).await.unwrap().iter().map(|v| v.key.len() + v.value.len() + 2).sum();
    assert!(total <= MAX_ENV_TOTAL_BYTES);

    // combined env: a group that fits alone but not with the service's own vars
    let group_vars: Vec<Value> = (0..5).map(|j| json!({"key": format!("G{j}"), "value": chunk.clone()})).collect();
    let r = app.post("/api/v1/env-groups", json!({"name": "fat", "vars": group_vars})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", &r.text()[..200.min(r.text().len())]);
    let r = app.post("/api/v1/services/probe/env-groups", json!({"group": "fat"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("service 'probe'"), "{}", r.text());
    let own: Vec<Value> = (0..4).map(|j| json!({"key": format!("O{j}"), "value": chunk.clone()})).collect();
    let r = app.post("/api/v1/services", json!({"name": "y", "env": own, "env_groups": ["fat"]})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // ...and growing a linked group is checked against every linked service
    let own: Vec<Value> = (0..3).map(|j| json!({"key": format!("O{j}"), "value": chunk.clone()})).collect();
    app.create_service(json!({"name": "small", "env": own, "env_groups": ["fat"]})).await;
    let r = app.patch("/api/v1/env-groups/fat/env", json!({"set": [{"key": "G9", "value": chunk.clone()}]})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("service 'small'"), "{}", r.text());
}

#[tokio::test]
async fn env_changes_during_the_first_deploy_queue_a_restart() {
    let app = TestApp::new().await;
    // an image service: its `create` deploy is still queued (not live)
    let v = app.create_service(json!({"name": "racey", "image": "python:3.12-alpine", "port": 8000})).await;
    let id = v["id"].as_str().unwrap().to_string();
    let r = app
        .patch("/api/v1/services/racey/env?restart=true", json!({"set": [{"key": "VERSION", "value": "two"}]}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {id} env_change")]);

    // env groups linked to it too
    app.post("/api/v1/env-groups", json!({"name": "g"})).await;
    app.post("/api/v1/services/racey/env-groups", json!({"group": "g"})).await;
    app.engine.clear();
    app.patch("/api/v1/env-groups/g/env?restart=true", json!({"set": [{"key": "X", "value": "1"}]})).await;
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {id} env_change")]);

    // once nothing is in flight and nothing is live, there's nothing to restart
    for d in app.store.active_deploys().await.unwrap() {
        app.store.set_deploy_status(&d.id, ferry_core::DeployStatus::Canceled, None).await.unwrap();
    }
    app.engine.clear();
    app.patch("/api/v1/services/racey/env?restart=true", json!({"set": [{"key": "VERSION", "value": "three"}]})).await;
    assert!(app.engine.calls_with("restart").is_empty());
}

#[tokio::test]
async fn deleting_a_linked_env_group_needs_force_and_can_restart() {
    let app = TestApp::new().await;
    app.post("/api/v1/env-groups", json!({"name": "shared", "vars": [{"key": "G1", "value": "one"}]})).await;
    let e1 = app.create_service(json!({"name": "echo1", "env_groups": ["shared"]})).await;
    app.create_service(json!({"name": "echo2", "env_groups": ["shared"]})).await;
    app.make_live("echo1", Some(80)).await;
    let r = app.delete("/api/v1/env-groups/shared").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(msg.contains("'echo1'") && msg.contains("'echo2'"), "{msg}");
    app.engine.clear();
    let r = app.delete("/api/v1/env-groups/shared?force=true&restart=true").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    // only the live one restarts, and without the group's variables
    assert_eq!(app.engine.calls_with("restart"), vec![format!("restart {} env_change", e1["id"].as_str().unwrap())]);
    assert!(app.store.effective_env(e1["id"].as_str().unwrap()).await.unwrap().is_empty());
    // unlinked groups delete without force
    app.post("/api/v1/env-groups", json!({"name": "lonely"})).await;
    assert_eq!(app.delete("/api/v1/env-groups/lonely").await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn deleting_a_referenced_datastore_needs_force() {
    let app = TestApp::new().await;
    app.post("/api/v1/datastores", json!({"name": "pg2", "kind": "postgres"})).await;
    app.post("/api/v1/datastores", json!({"name": "unused", "kind": "redis"})).await;
    app.create_service(
        json!({"name": "echo2", "env": [{"key": "PG2", "value": "${{datastore.pg2.connectionString}}"}]}),
    )
    .await;
    // through an env group as well
    app.post("/api/v1/env-groups", json!({"name": "db", "vars": [{"key": "DB_HOST", "value": "${{db.pg2.host}}"}]}))
        .await;
    app.create_service(json!({"name": "api", "env_groups": ["db"]})).await;
    app.engine.clear();
    let r = app.delete("/api/v1/datastores/pg2").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let msg = r.json()["error"]["message"].as_str().unwrap().to_string();
    assert!(msg.contains("service 'echo2' (PG2)") && msg.contains("service 'api' (DB_HOST)"), "{msg}");
    assert!(app.engine.calls().is_empty());
    assert_eq!(app.delete("/api/v1/datastores/unused").await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.delete("/api/v1/datastores/pg2?force=1").await.status, StatusCode::NO_CONTENT);
}
