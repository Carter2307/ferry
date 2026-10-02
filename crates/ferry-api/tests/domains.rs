//! The domains services are served under (`/api/v1/domains`) and the
//! certificates of the routed hosts (`/api/v1/certificates`), DESIGN.md §21.

mod common;

use common::TestApp;
use ferry_core::tls::CertificateStatus;
use ferry_core::{Domain, DomainSource};
use http::StatusCode;
use serde_json::{Value, json};

fn names(list: &Value) -> Vec<&str> {
    list.as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect()
}

async fn connect(app: &TestApp, name: &str) -> Value {
    let r = app.post("/api/v1/domains", json!({ "name": name })).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    r.json()
}

#[tokio::test]
async fn the_base_domain_is_a_domain_only_the_flag_removes() {
    let app = TestApp::new().await;
    let list = app.get("/api/v1/domains").await.json();
    assert_eq!(names(&list), vec!["localhost"]);
    let base = &list[0];
    assert_eq!((base["source"].as_str(), base["status"].as_str()), (Some("config"), Some("active")));
    assert_eq!(base["is_default"], true);
    assert_eq!((base["local"].as_bool(), base["served"].as_bool()), (Some(true), Some(true)));
    assert_eq!(base["records"], json!([]));
    assert_eq!(base["url_pattern"], "http://<service>.localhost:8080");
    assert!(base.get("failures").is_none(), "{base}");
    // A local name has no record: the server's address was not looked up.
    assert!(app.engine.calls_with("public_addresses").is_empty());

    // By name or by id.
    let id = base["id"].as_str().unwrap();
    assert!(id.starts_with("dom-"));
    assert_eq!(app.get(&format!("/api/v1/domains/{id}")).await.json(), *base);
    assert_eq!(app.get("/api/v1/domains/LocalHost").await.json(), *base);
    assert_eq!(app.get("/api/v1/domains/nope.example.com").await.status, StatusCode::NOT_FOUND);

    let r = app.delete("/api/v1/domains/localhost").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("--base-domain"), "{}", r.text());
    let r = app.patch("/api/v1/domains/localhost", json!({"is_default": false})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Nothing to change: the domain as it is.
    assert_eq!(app.patch("/api/v1/domains/localhost", json!({})).await.json(), *base);
    assert_eq!(app.patch("/api/v1/domains/localhost", json!({"is_default": true})).await.json(), *base);
    assert!(app.engine.calls_with("refresh_domains").is_empty());
}

#[tokio::test]
async fn a_connected_domain_is_served_once_it_is_verified() {
    let app = TestApp::new().await;
    let web = app.create_service(json!({"name": "web"})).await;
    assert_eq!(web["hosts"], json!(["web.localhost"]));

    // Connected: pending, with the records that point it at the server.
    let d = connect(&app, " Example.COM. ").await;
    assert_eq!(
        (d["name"].as_str(), d["source"].as_str(), d["status"].as_str()),
        (Some("example.com"), Some("connected"), Some("pending"))
    );
    assert_eq!(
        (d["local"].as_bool(), d["served"].as_bool(), d["is_default"].as_bool()),
        (Some(false), Some(false), Some(false))
    );
    assert_eq!(
        d["records"],
        json!([
            {"type": "A", "name": "*", "value": "203.0.113.10", "required": true},
            {"type": "A", "name": "@", "value": "203.0.113.10", "required": false},
        ])
    );
    assert_eq!(d["url_pattern"], "http://<service>.example.com:8080");
    assert_eq!(d["checks"], json!([]));
    assert_eq!(app.engine.calls_with("refresh_domains").len(), 1);
    assert_eq!(names(&app.get("/api/v1/domains").await.json()), vec!["localhost", "example.com"]);

    // Not served yet, but its names are spoken for.
    assert_eq!(app.get("/api/v1/services/web").await.json()["hosts"], json!(["web.localhost"]));
    app.create_service(json!({"name": "api"})).await;
    let r = app.post("/api/v1/services/web/domains", json!({"domain": "api.example.com"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("default host of service 'api'"), "{}", r.text());
    let r = app.patch("/api/v1/domains/example.com", json!({"is_default": true})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("not served yet"), "{}", r.text());

    // Verified too early: what was found, and still pending.
    let v = app.post("/api/v1/domains/example.com/verify", json!({})).await.json();
    assert_eq!((v["status"].as_str(), v["checks"][0]["outcome"].as_str()), (Some("pending"), Some("failed")));
    assert_eq!(v["checks"][0]["kind"], "dns");
    assert!(v["checked_at"].is_string() && v["verified_at"].is_null());

    // Its DNS points at the server: active, served, and the default domain
    // (the default one was a local name).
    app.engine.reachable.lock().unwrap().push("example.com".into());
    let v = app.post(&format!("/api/v1/domains/{}/verify", d["id"].as_str().unwrap()), json!({})).await.json();
    assert_eq!(
        (v["status"].as_str(), v["served"].as_bool(), v["is_default"].as_bool()),
        (Some("active"), Some(true), Some(true))
    );
    assert!(v["verified_at"].is_string());
    let web = app.get("/api/v1/services/web").await.json();
    assert_eq!(web["hosts"], json!(["web.example.com", "web.localhost"]));
    assert_eq!(web["url"], "http://web.example.com:8080");
    let info = app.get("/api/v1/info").await.json();
    assert_eq!(
        (info["base_domain"].as_str(), info["default_domain"].as_str()),
        (Some("localhost"), Some("example.com"))
    );
    assert_eq!(names(&app.get("/api/v1/domains").await.json()), vec!["example.com", "localhost"]);

    // Another default: the URL of the services follows.
    let r = app.patch("/api/v1/domains/localhost", json!({"is_default": true})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["is_default"], true);
    let web = app.get("/api/v1/services/web").await.json();
    assert_eq!(web["url"], "http://web.localhost:8080");
    assert_eq!(web["hosts"], json!(["web.localhost", "web.example.com"]));
    assert_eq!(app.patch("/api/v1/domains/example.com", json!({"is_default": true})).await.json()["is_default"], true);

    // Disconnected: its hostnames go, and the base domain is the default again.
    app.engine.clear();
    assert_eq!(app.delete("/api/v1/domains/example.com").await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.engine.calls_with("refresh_domains").len(), 1);
    let web = app.get("/api/v1/services/web").await.json();
    assert_eq!(
        (web["hosts"].clone(), web["url"].as_str()),
        (json!(["web.localhost"]), Some("http://web.localhost:8080"))
    );
    let list = app.get("/api/v1/domains").await.json();
    assert_eq!(names(&list), vec!["localhost"]);
    assert_eq!(list[0]["is_default"], true);
    assert_eq!(app.delete("/api/v1/domains/example.com").await.status, StatusCode::NOT_FOUND);
    // Its names are free again.
    let r = app.post("/api/v1/services/web/domains", json!({"domain": "api.example.com"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
}

#[tokio::test]
async fn names_that_are_not_domains_and_duplicates_are_refused() {
    let app = TestApp::new().await;
    for (bad, hint) in [
        ("*.example.com", "<service>.example.com"),
        ("https://example.com", "URL"),
        ("example.com/app", "URL"),
        ("localhost", "invalid domain"),
        ("203.0.113.10", "IP address"),
        ("", "invalid domain"),
    ] {
        let r = app.post("/api/v1/domains", json!({ "name": bad })).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.text());
        assert!(r.text().contains(hint), "{bad}: {}", r.text());
    }
    assert_eq!(app.post("/api/v1/domains", json!({"domain": "example.com"})).await.status, StatusCode::BAD_REQUEST);

    connect(&app, "example.com").await;
    let r = app.post("/api/v1/domains", json!({"name": "EXAMPLE.com"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("already connected"), "{}", r.text());
    let app2 = TestApp::with_config(|c| c.base_domain = "apps.example.com".into()).await;
    let r = app2.post("/api/v1/domains", json!({"name": "apps.example.com"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("base domain"), "{}", r.text());

    // A domain must not put a service on the custom domain of another one.
    let app = TestApp::new().await;
    app.create_service(json!({"name": "a", "custom_domains": ["b.example.org", "a.example.org"]})).await;
    app.create_service(json!({"name": "b"})).await;
    let r = app.post("/api/v1/domains", json!({"name": "example.org"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("'b.example.org'") && r.text().contains("service 'a'"), "{}", r.text());
    assert_eq!(names(&app.get("/api/v1/domains").await.json()), vec!["localhost"]);

    // At most 20 connected domains.
    for i in 0..ferry_core::domains::MAX_DOMAINS {
        app.store.create_domain(&Domain::new(format!("d{i}.example.net"), DomainSource::Connected)).await.unwrap();
    }
    let r = app.post("/api/v1/domains", json!({"name": "one-more.example.net"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("20 connected domains"), "{}", r.text());
}

#[tokio::test]
async fn a_public_base_domain_is_served_whatever_its_verification_says() {
    let app = TestApp::with_config(|c| c.base_domain = "apps.example.com".into()).await;
    let web = app.create_service(json!({"name": "web"})).await;
    assert_eq!(web["hosts"], json!(["web.apps.example.com"]));
    let base = app.get("/api/v1/domains/apps.example.com").await.json();
    assert_eq!((base["source"].as_str(), base["status"].as_str()), (Some("config"), Some("pending")));
    assert_eq!((base["served"].as_bool(), base["is_default"].as_bool()), (Some(true), Some(true)));
    assert_eq!(base["records"][0], json!({"type": "A", "name": "*", "value": "203.0.113.10", "required": true}));
    // A server that can't tell its address still says which records to create.
    app.engine.public_ips.lock().unwrap().clear();
    let base = app.get("/api/v1/domains/apps.example.com").await.json();
    assert_eq!(base["records"][0], json!({"type": "A", "name": "*", "value": null, "required": true}));
}

#[tokio::test]
async fn local_domains_are_served_at_once() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web"})).await;
    let d = connect(&app, "dev.localhost").await;
    assert_eq!(
        (d["status"].as_str(), d["local"].as_bool(), d["served"].as_bool()),
        (Some("active"), Some(true), Some(true))
    );
    assert_eq!(d["records"], json!([]));
    // Not the default: the default one is as local as this one.
    assert_eq!(d["is_default"], false);
    assert_eq!(app.get("/api/v1/services/web").await.json()["hosts"], json!(["web.localhost", "web.dev.localhost"]));
    // Nothing to verify.
    let v = app.post("/api/v1/domains/dev.localhost/verify", json!({})).await.json();
    assert_eq!((v["status"].as_str(), v["checked_at"].is_null()), (Some("active"), true));
    assert!(app.engine.calls_with("public_addresses").is_empty());
}

#[tokio::test]
async fn certificates_of_the_routed_hosts() {
    // Without HTTPS: every public host says so; local ones never get one.
    let app = TestApp::new().await;
    app.create_service(json!({"name": "web", "custom_domains": ["app.example.com"]})).await;
    app.create_service(json!({"name": "bg", "type": "worker"})).await;
    let list = app.get("/api/v1/certificates").await.json();
    let rows: Vec<(&str, Option<&str>, &str)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["host"].as_str().unwrap(), c["service"].as_str(), c["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("ferry.localhost", None, "local"),
            ("web.localhost", Some("web"), "local"),
            ("app.example.com", Some("web"), "disabled"),
        ]
    );

    // With HTTPS: where each certificate stands.
    let app = TestApp::with_tls().await;
    app.create_service(json!({"name": "web", "custom_domains": ["app.example.com", "old.example.com"]})).await;
    app.engine.reachable.lock().unwrap().push("example.org".into());
    connect(&app, "example.org").await;
    app.post("/api/v1/domains/example.org/verify", json!({})).await;
    let expires = ferry_core::now() + chrono::Duration::days(60);
    {
        let manager = app.certificates.as_ref().unwrap();
        let mut states = manager.0.lock().unwrap();
        states.insert("web.example.org".into(), CertificateStatus::Issued { not_after: expires });
        states.insert("app.example.com".into(), CertificateStatus::Issuing);
        states.insert(
            "old.example.com".into(),
            CertificateStatus::Failed { error: "no valid A record found".into(), retry_at: Some(expires) },
        );
    }
    let list = app.get("/api/v1/certificates").await.json();
    let by_host = |host: &str| list.as_array().unwrap().iter().find(|c| c["host"] == host).unwrap().clone();
    assert_eq!(by_host("ferry.localhost")["state"], "local");
    let issued = by_host("web.example.org");
    assert_eq!((issued["state"].as_str(), issued["service"].as_str()), (Some("issued"), Some("web")));
    assert!(issued["expires_at"].is_string() && issued["error"].is_null());
    assert_eq!(by_host("app.example.com")["state"], "issuing");
    let failed = by_host("old.example.com");
    assert_eq!((failed["state"].as_str(), failed["error"].as_str()), (Some("failed"), Some("no valid A record found")));
    assert!(failed["retry_at"].is_string());
    assert_eq!(by_host("web.localhost")["state"], "local");
    // The URL of the service is under the verified domain, in HTTPS.
    assert_eq!(app.get("/api/v1/services/web").await.json()["url"], "https://web.example.org");
    assert_eq!(app.get("/api/v1/domains/example.org").await.json()["url_pattern"], "https://<service>.example.org");
}
