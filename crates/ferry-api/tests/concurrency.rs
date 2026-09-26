//! Concurrent requests must not lose acknowledged changes or break
//! uniqueness rules (run on a multi-threaded runtime so requests really
//! interleave).

mod common;

use std::sync::Arc;

use common::TestApp;
use http::{Method, StatusCode};
use serde_json::{Value, json};

/// Fire all requests at once; returns the statuses in request order.
async fn concurrently(app: &Arc<TestApp>, reqs: Vec<(Method, String, Value)>) -> Vec<StatusCode> {
    let handles: Vec<_> = reqs
        .into_iter()
        .map(|(method, uri, body)| {
            let app = app.clone();
            tokio::spawn(async move { app.call(method, &uri, Some(body)).await.status })
        })
        .collect();
    let mut out = Vec::new();
    for h in handles {
        out.push(h.await.unwrap());
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parallel_domain_adds_are_all_kept() {
    let app = Arc::new(TestApp::new().await);
    app.create_service(json!({"name": "www"})).await;
    for round in 0..5 {
        let reqs = (0..10)
            .map(|i| {
                (
                    Method::POST,
                    "/api/v1/services/www/domains".to_string(),
                    json!({"domain": format!("d{round}-{i}.example.com")}),
                )
            })
            .collect();
        let statuses = concurrently(&app, reqs).await;
        assert!(statuses.iter().all(|s| *s == StatusCode::OK), "{statuses:?}");
    }
    let domains = app.get("/api/v1/services/www/domains").await.json();
    assert_eq!(domains.as_array().unwrap().len(), 50, "{domains}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parallel_patches_of_different_fields_are_all_kept() {
    let app = Arc::new(TestApp::new().await);
    app.create_service(json!({"name": "web"})).await;
    for n in 0..10 {
        let reqs = vec![
            (Method::PATCH, "/api/v1/services/web".to_string(), json!({"build_command": format!("b{n}")})),
            (Method::PATCH, "/api/v1/services/web".to_string(), json!({"start_command": format!("s{n}")})),
            (Method::PATCH, "/api/v1/services/web".to_string(), json!({"root_dir": format!("r{n}")})),
            (Method::POST, "/api/v1/services/web/domains".to_string(), json!({"domain": format!("p{n}.example.com")})),
        ];
        let statuses = concurrently(&app, reqs).await;
        assert!(statuses.iter().all(|s| *s == StatusCode::OK), "{statuses:?}");
        let svc = app.store.require_service("web").await.unwrap();
        assert_eq!(
            (svc.build_command.as_deref(), svc.start_command.as_deref(), svc.root_dir.as_deref()),
            (Some(format!("b{n}").as_str()), Some(format!("s{n}").as_str()), Some(format!("r{n}").as_str())),
            "round {n}"
        );
        assert!(svc.custom_domains.contains(&format!("p{n}.example.com")), "round {n}: {:?}", svc.custom_domains);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn suspend_is_not_undone_by_a_parallel_domain_add() {
    let app = Arc::new(TestApp::new().await);
    app.create_service(json!({"name": "t3"})).await;
    for round in 0..10 {
        app.store.set_suspended(&app.store.require_service("t3").await.unwrap().id, false).await.unwrap();
        let mut reqs: Vec<(Method, String, Value)> = (0..4)
            .map(|i| {
                (
                    Method::POST,
                    "/api/v1/services/t3/domains".to_string(),
                    json!({"domain": format!("s{round}-{i}.example.com")}),
                )
            })
            .collect();
        reqs.insert(2, (Method::POST, "/api/v1/services/t3/suspend".to_string(), json!({})));
        let statuses = concurrently(&app, reqs).await;
        assert!(statuses.iter().all(|s| *s == StatusCode::OK), "{statuses:?}");
        assert!(app.store.require_service("t3").await.unwrap().suspended, "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_custom_domain_is_claimed_by_exactly_one_service() {
    let app = Arc::new(TestApp::new().await);
    let names = ["t3", "t4", "api", "www", "dash", "hooks", "health"];
    for n in names.iter().chain(&["patcher"]) {
        app.create_service(json!({"name": n})).await;
    }
    for round in 0..5 {
        let domain = format!("race{round}.example.com");
        let mut reqs: Vec<(Method, String, Value)> = names
            .iter()
            .map(|n| (Method::POST, format!("/api/v1/services/{n}/domains"), json!({ "domain": domain })))
            .collect();
        // PATCH and create race for the same host too (the PATCH targets a
        // service that doesn't also add the domain: re-setting a domain a
        // service already owns is a legitimate success)
        reqs.push((Method::PATCH, "/api/v1/services/patcher".to_string(), json!({"custom_domains": [domain]})));
        reqs.push((
            Method::POST,
            "/api/v1/services".to_string(),
            json!({"name": format!("new{round}"), "custom_domains": [domain]}),
        ));
        let statuses = concurrently(&app, reqs).await;
        let ok = statuses.iter().filter(|s| s.is_success()).count();
        assert_eq!(ok, 1, "round {round}: {statuses:?}");
        assert!(statuses.iter().all(|s| s.is_success() || *s == StatusCode::CONFLICT), "{statuses:?}");
        let owners: Vec<String> = app
            .store
            .list_services()
            .await
            .unwrap()
            .into_iter()
            .filter(|s| s.custom_domains.contains(&domain))
            .map(|s| s.name)
            .collect();
        assert_eq!(owners.len(), 1, "round {round}: {owners:?}");
    }
    // a new service whose default host races with a custom domain claim
    for round in 0..5 {
        let host = format!("fresh{round}.localhost");
        let reqs = vec![
            (Method::POST, "/api/v1/services".to_string(), json!({"name": format!("fresh{round}")})),
            (Method::POST, "/api/v1/services/www/domains".to_string(), json!({ "domain": host })),
        ];
        let statuses = concurrently(&app, reqs).await;
        assert_eq!(statuses.iter().filter(|s| s.is_success()).count(), 1, "round {round}: {statuses:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parallel_env_patches_are_all_kept() {
    let app = Arc::new(TestApp::new().await);
    app.create_service(json!({"name": "web"})).await;
    app.post("/api/v1/env-groups", json!({"name": "g"})).await;
    let reqs = (0..20)
        .flat_map(|i| {
            [
                (
                    Method::PATCH,
                    "/api/v1/services/web/env".to_string(),
                    json!({"set": [{"key": format!("K{i}"), "value": "v"}]}),
                ),
                (
                    Method::PATCH,
                    "/api/v1/env-groups/g/env".to_string(),
                    json!({"set": [{"key": format!("K{i}"), "value": "v"}]}),
                ),
            ]
        })
        .collect();
    let statuses = concurrently(&app, reqs).await;
    assert!(statuses.iter().all(|s| *s == StatusCode::OK), "{statuses:?}");
    assert_eq!(app.get("/api/v1/services/web/env").await.json().as_array().unwrap().len(), 20);
    assert_eq!(app.get("/api/v1/env-groups/g").await.json()["vars"].as_array().unwrap().len(), 20);
}
