//! Blueprints: parsing real files, apply / re-apply / update, dry runs.

mod common;

use axum::body::Body;
use common::{TOKEN, TestApp};
use ferry_api::blueprint;
use ferry_core::{DatastoreKind, Runtime, ServiceType};
use http::{Method, Request, StatusCode};
use serde_json::{Value, json};

const EXAMPLE: &str = include_str!("../../../examples/blueprint/ferry.yaml");

/// A realistic Render blueprint using many keys Ferry doesn't support.
const RENDER_YAML: &str = r#"
services:
  # A Node.js web service
  - type: web
    name: webapp
    runtime: node
    repo: https://github.com/render-examples/express-hello-world
    plan: starter
    region: oregon
    branch: main
    buildCommand: yarn
    startCommand: node app.js
    healthCheckPath: /healthz
    autoDeploy: false
    numInstances: 2
    scaling:
      minInstances: 1
      maxInstances: 3
      targetMemoryPercent: 60
    domains:
      - app.example.com
    envVars:
      - key: NODE_ENV
        value: production
      - key: API_KEY
        generateValue: true
      - key: DATABASE_URL
        fromDatabase:
          name: mydatabase
          property: connectionString
      - key: REDIS_URL
        fromService:
          type: redis
          name: lightning
          property: connectionString
      - key: MINIO_HOST
        fromService:
          type: pserv
          name: minio
          property: hostport
      - key: MINIO_PASSWORD
        fromService:
          type: pserv
          name: minio
          envVarKey: MINIO_ROOT_PASSWORD
      - fromGroup: conc-settings
      - key: STRIPE_API_KEY
        sync: false

  - type: pserv
    name: minio
    env: docker
    repo: https://github.com/render-examples/minio
    dockerfilePath: ./Dockerfile
    dockerContext: .
    disk:
      name: data
      mountPath: /data
      sizeGB: 10
    envVars:
      - key: MINIO_ROOT_PASSWORD
        generateValue: true
      - key: PORT
        value: 10000

  - type: worker
    name: queue
    runtime: python
    repo: https://github.com/render-examples/celery
    buildCommand: pip install -r requirements.txt
    startCommand: celery --app tasks worker --loglevel info
    envVars:
      - key: CELERY_BROKER_URL
        fromService:
          name: celery-redis
          type: keyvalue
          property: connectionString

  - type: cron
    name: date
    runtime: docker
    repo: https://github.com/render-examples/docker-cron.git
    schedule: "0 0 * * *"
    dockerCommand: date

  - type: web
    name: site
    runtime: static
    buildCommand: yarn build
    staticPublishPath: ./build
    pullRequestPreviewsEnabled: true
    headers:
      - path: /*
        name: X-Frame-Options
        value: sameorigin
    routes:
      - type: rewrite
        source: /*
        destination: /index.html

  - type: redis
    name: lightning
    ipAllowList: []
    plan: free
    maxmemoryPolicy: allkeys-lru

  - type: keyvalue
    name: celery-redis
    region: frankfurt
    plan: starter
    ipAllowList:
      - source: 0.0.0.0/0
        description: everywhere

databases:
  - name: mydatabase
    databaseName: mydb
    user: mydbuser
    plan: pro
    region: frankfurt
    ipAllowList: []
    postgresMajorVersion: "15"

envVarGroups:
  - name: conc-settings
    envVars:
      - key: CONCURRENCY
        value: 2
      - key: SECRET
        generateValue: true
  - name: stripe
    envVars:
      - key: STRIPE_API_URL
        value: https://api.stripe.com/v2

previews:
  generation: automatic
"#;

async fn apply(app: &TestApp, yaml: &str, dry_run: bool) -> (StatusCode, Value) {
    let r = app.post("/api/v1/blueprints/apply", json!({"yaml": yaml, "dry_run": dry_run})).await;
    (r.status, r.json())
}

fn actions(result: &Value) -> Vec<(String, String, String)> {
    result["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["resource"].as_str().unwrap().to_string(),
                a["name"].as_str().unwrap().to_string(),
                a["action"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn action_of<'a>(result: &'a Value, name: &str) -> &'a Value {
    result["actions"].as_array().unwrap().iter().find(|a| a["name"] == name).unwrap()
}

async fn env_of(app: &TestApp, service: &str) -> Vec<(String, String)> {
    let svc = app.store.require_service(service).await.unwrap();
    app.store.list_env(&svc.id).await.unwrap().into_iter().map(|v| (v.key, v.value)).collect()
}

fn get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[test]
fn parses_the_example_blueprint() {
    let bp = blueprint::parse(EXAMPLE).unwrap();
    assert!(bp.warnings.is_empty(), "{:?}", bp.warnings);
    assert_eq!(bp.env_groups.len(), 1);
    assert_eq!(bp.datastores.len(), 2);
    let db = bp.datastores.iter().find(|d| d.name == "app-db").unwrap();
    assert_eq!(db.kind, DatastoreKind::Postgres);
    assert_eq!(db.version.as_deref(), Some("16"));
    assert_eq!(db.database.as_deref(), Some("app"));
    assert_eq!(db.user.as_deref(), Some("app"));
    assert_eq!(bp.datastores.iter().find(|d| d.name == "cache").unwrap().kind, DatastoreKind::Redis);
    let hello = bp.services.iter().find(|s| s.name == "hello").unwrap();
    assert_eq!(hello.service_type, ServiceType::WebService);
    assert_eq!(hello.runtime, Some(Runtime::Image));
    assert_eq!(hello.image.as_deref(), Some("nginx:alpine"));
    assert_eq!(hello.port, Some(80));
    assert_eq!(hello.instances, Some(2));
    assert_eq!(hello.env_groups, vec!["shared-settings"]);
    assert_eq!(hello.env_vars.len(), 3);
    let nightly = bp.services.iter().find(|s| s.name == "nightly").unwrap();
    assert_eq!(nightly.service_type, ServiceType::CronJob);
    assert_eq!(nightly.schedule.as_deref(), Some("0 3 * * *"));
    assert!(nightly.start_command.as_deref().unwrap().starts_with("echo"));
}

#[test]
fn parses_a_render_yaml_with_warnings() {
    let bp = blueprint::parse(RENDER_YAML).unwrap();
    assert_eq!(bp.services.len(), 5);
    assert_eq!(bp.datastores.len(), 3);
    assert_eq!(bp.env_groups.len(), 2);
    for key in [
        "plan",
        "region",
        "scaling",
        "previews",
        "headers",
        "routes",
        "ipAllowList",
        "maxmemoryPolicy",
        "pullRequestPreviewsEnabled",
    ] {
        assert!(bp.warnings.iter().any(|w| w.contains(&format!("'{key}'"))), "no warning for {key}: {:?}", bp.warnings);
    }
    let site = bp.services.iter().find(|s| s.name == "site").unwrap();
    assert_eq!(site.service_type, ServiceType::StaticSite);
    let minio = bp.services.iter().find(|s| s.name == "minio").unwrap();
    assert_eq!(minio.runtime, Some(Runtime::Docker));
    assert_eq!(minio.disk_mount_path.as_deref(), Some("/data"));
    assert_eq!(minio.root_dir, None);
    let date = bp.services.iter().find(|s| s.name == "date").unwrap();
    assert_eq!(date.start_command.as_deref(), Some("date"));
}

#[tokio::test]
async fn apply_render_yaml_then_reapply_then_modify() {
    let app = TestApp::new().await;

    // ---- first apply: everything is created ---------------------------------
    let (status, res) = apply(&app, RENDER_YAML, false).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["dry_run"], false);
    let acts = actions(&res);
    assert_eq!(
        acts.iter().map(|(r, n, a)| format!("{r}:{n}:{a}")).collect::<Vec<_>>(),
        [
            "env_group:conc-settings:create",
            "env_group:stripe:create",
            "datastore:mydatabase:create",
            "datastore:lightning:create",
            "datastore:celery-redis:create",
            "service:webapp:create",
            "service:minio:create",
            "service:queue:create",
            "service:date:create",
            "service:site:create",
        ]
    );
    let warnings: Vec<&str> = res["warnings"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
    assert!(warnings.iter().any(|w| w.contains("site") && w.contains("ferry up site")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("STRIPE_API_KEY") && w.contains("sync: false")), "{warnings:?}");

    // blueprint deploys for new services with a source, in order
    let deploys = res["deploys"].as_array().unwrap();
    assert_eq!(deploys.len(), 4);
    assert!(deploys.iter().all(|d| d["trigger"] == "blueprint"));
    assert_eq!(app.engine.calls_with("provision").len(), 3);

    // datastores
    let db = app.store.require_datastore("mydatabase").await.unwrap();
    assert_eq!((db.version.as_str(), db.database.as_deref(), db.username.as_str()), ("15", Some("mydb"), "mydbuser"));
    assert_eq!(app.store.require_datastore("lightning").await.unwrap().kind, DatastoreKind::Redis);

    // services
    let web = app.store.require_service("webapp").await.unwrap();
    assert_eq!(web.runtime, Runtime::Node);
    assert_eq!(web.instances, 2);
    assert!(!web.auto_deploy);
    assert_eq!(web.custom_domains, vec!["app.example.com"]);
    assert_eq!(web.health_check_path.as_deref(), Some("/healthz"));
    let minio = app.store.require_service("minio").await.unwrap();
    assert_eq!(minio.service_type, ServiceType::PrivateService);
    assert_eq!(minio.disk_mount_path.as_deref(), Some("/data"));
    let site = app.store.require_service("site").await.unwrap();
    assert_eq!((site.service_type, site.runtime), (ServiceType::StaticSite, Runtime::Static));
    assert_eq!(site.publish_dir.as_deref(), Some("./build"));

    // env vars: references, generated values, copies, groups
    let env = env_of(&app, "webapp").await;
    assert_eq!(get(&env, "NODE_ENV"), Some("production"));
    assert_eq!(get(&env, "DATABASE_URL"), Some("${{datastore.mydatabase.connectionString}}"));
    assert_eq!(get(&env, "REDIS_URL"), Some("${{datastore.lightning.connectionString}}"));
    assert_eq!(get(&env, "MINIO_HOST"), Some("${{service.minio.hostport}}"));
    assert_eq!(get(&env, "STRIPE_API_KEY"), None);
    let api_key = get(&env, "API_KEY").unwrap().to_string();
    assert_eq!(api_key.len(), 64);
    let minio_env = env_of(&app, "minio").await;
    assert_eq!(get(&env, "MINIO_PASSWORD"), get(&minio_env, "MINIO_ROOT_PASSWORD"));
    assert_eq!(get(&minio_env, "PORT"), Some("10000"));
    assert_eq!(
        get(&env_of(&app, "queue").await, "CELERY_BROKER_URL"),
        Some("${{datastore.celery-redis.connectionString}}")
    );
    let groups = app.store.service_env_groups(&web.id).await.unwrap();
    assert_eq!(groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["conc-settings"]);
    let conc = app.store.require_env_group("conc-settings").await.unwrap();
    let group_env = app.store.list_env(&conc.id).await.unwrap();
    let group_secret = group_env.iter().find(|v| v.key == "SECRET").unwrap().value.clone();

    // the compiled references resolve with the real resolver
    let (dss, refs) = app.store.reference_targets(&app.config).await.unwrap();
    let ctx = ferry_core::env::RefContext { datastores: &dss, services: &refs, advertise_host: "127.0.0.1" };
    let resolved = ferry_core::env::resolve_value(get(&env, "DATABASE_URL").unwrap(), &ctx).unwrap();
    assert!(resolved.starts_with("postgresql://mydbuser:"), "{resolved}");

    // ---- re-apply: nothing changes -------------------------------------------
    app.engine.clear();
    let (status, res) = apply(&app, RENDER_YAML, false).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert!(actions(&res).iter().all(|(_, _, a)| a == "unchanged"), "{res}");
    assert_eq!(res["deploys"], json!([]));
    assert!(app.engine.calls().is_empty(), "{:?}", app.engine.calls());
    // generated values are never rotated
    assert_eq!(get(&env_of(&app, "webapp").await, "API_KEY"), Some(api_key.as_str()));
    let group_env = app.store.list_env(&conc.id).await.unwrap();
    assert_eq!(group_env.iter().find(|v| v.key == "SECRET").unwrap().value, group_secret);

    // ---- modify: settings, env, group --------------------------------------------
    app.make_live("webapp", Some(3000)).await;
    app.make_live("queue", None).await;
    app.make_live("minio", Some(9000)).await;
    app.engine.clear();
    let modified = RENDER_YAML
        .replace("startCommand: node app.js", "startCommand: node server.js")
        .replace("numInstances: 2", "numInstances: 3")
        .replace("--loglevel info", "--loglevel debug")
        .replace("        value: 2\n", "        value: 4\n")
        .replace(
            "      - key: PORT\n        value: 10000",
            "      - key: PORT\n        value: 10000\n      - key: EXTRA\n        value: yes",
        );
    let (status, res) = apply(&app, &modified, false).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let by_name = |n: &str| actions(&res).into_iter().find(|(_, name, _)| name == n).unwrap().2;
    assert_eq!(by_name("conc-settings"), "update");
    assert_eq!(by_name("stripe"), "unchanged");
    assert_eq!(by_name("mydatabase"), "unchanged");
    assert_eq!(by_name("webapp"), "update");
    assert_eq!(by_name("queue"), "update");
    assert_eq!(by_name("minio"), "update");
    assert_eq!(by_name("date"), "unchanged");
    assert_eq!(action_of(&res, "conc-settings")["changes"], json!(["env CONCURRENCY: updated"]));
    assert_eq!(
        action_of(&res, "webapp")["changes"],
        json!(["start_command: node app.js → node server.js", "instances: 2 → 3"])
    );
    assert_eq!(action_of(&res, "minio")["changes"], json!(["env EXTRA: added"]));

    let web_id = app.store.require_service("webapp").await.unwrap().id;
    let queue_id = app.store.require_service("queue").await.unwrap().id;
    let minio_id = app.store.require_service("minio").await.unwrap().id;
    let calls = app.engine.calls();
    // webapp: build setting changed → blueprint deploy (+ scale); no restart
    assert!(calls.contains(&format!("scale {web_id} 3")), "{calls:?}");
    assert!(calls.iter().any(|c| c.starts_with(&format!("deploy {web_id} blueprint"))), "{calls:?}");
    assert!(!calls.contains(&format!("restart {web_id} env_change")), "{calls:?}");
    // queue: start command changed → deploy
    assert!(calls.iter().any(|c| c.starts_with(&format!("deploy {queue_id} blueprint"))), "{calls:?}");
    // minio: only env changed → restart
    assert!(calls.contains(&format!("restart {minio_id} env_change")), "{calls:?}");
    assert_eq!(res["deploys"].as_array().unwrap().len(), 3);
    assert_eq!(app.store.require_service("webapp").await.unwrap().instances, 3);
    assert_eq!(get(&env_of(&app, "minio").await, "EXTRA"), Some("yes"));
}

#[tokio::test]
async fn env_group_change_restarts_live_linked_services() {
    let app = TestApp::new().await;
    let yaml = "envVarGroups:\n  - name: shared\n    envVars:\n      - {key: A, value: '1'}\nservices:\n  - {type: worker, name: bg, image: busybox, envVars: [{fromGroup: shared}]}\n";
    assert_eq!(apply(&app, yaml, false).await.0, StatusCode::OK);
    // a service outside the blueprint linked to the group
    app.create_service(json!({"name": "outside", "env_groups": ["shared"]})).await;
    app.make_live("bg", None).await;
    app.make_live("outside", None).await;
    app.engine.clear();
    let (status, res) = apply(&app, &yaml.replace("'1'", "'2'"), false).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(action_of(&res, "shared")["action"], "update");
    assert_eq!(action_of(&res, "bg")["action"], "unchanged");
    let restarts = app.engine.calls_with("restart");
    assert_eq!(restarts.len(), 2, "{restarts:?}");
    assert_eq!(res["deploys"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn apply_the_example_blueprint() {
    let app = TestApp::new().await;
    let (status, res) = apply(&app, EXAMPLE, false).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["warnings"], json!([]));
    assert_eq!(res["deploys"].as_array().unwrap().len(), 2);
    let hello = app.store.require_service("hello").await.unwrap();
    assert_eq!((hello.instances, hello.port, hello.runtime), (2, Some(80), Runtime::Image));
    let env = env_of(&app, "hello").await;
    assert_eq!(get(&env, "DATABASE_URL"), Some("${{datastore.app-db.connectionString}}"));
    assert_eq!(get(&env, "REDIS_URL"), Some("${{datastore.cache.connectionString}}"));
    assert_eq!(get(&env, "SESSION_SECRET").unwrap().len(), 64);
    let nightly = app.store.require_service("nightly").await.unwrap();
    assert_eq!(nightly.service_type, ServiceType::CronJob);
    assert_eq!(app.store.require_datastore("app-db").await.unwrap().database.as_deref(), Some("app"));
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let app = TestApp::new().await;
    let (status, res) = apply(&app, RENDER_YAML, true).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["dry_run"], true);
    assert_eq!(res["deploys"], json!([]));
    assert_eq!(actions(&res).len(), 10);
    assert!(actions(&res).iter().all(|(_, _, a)| a == "create"));
    assert!(app.store.list_services().await.unwrap().is_empty());
    assert!(app.store.list_datastores().await.unwrap().is_empty());
    assert!(app.store.list_env_groups().await.unwrap().is_empty());
    assert!(app.engine.calls().is_empty());

    // a dry run over existing state reports updates without applying them
    assert_eq!(apply(&app, RENDER_YAML, false).await.0, StatusCode::OK);
    app.engine.clear();
    let modified = RENDER_YAML.replace("healthCheckPath: /healthz", "healthCheckPath: /ready");
    let (_, res) = apply(&app, &modified, true).await;
    assert_eq!(action_of(&res, "webapp")["changes"], json!(["health_check_path: /healthz → /ready"]));
    assert_eq!(app.store.require_service("webapp").await.unwrap().health_check_path.as_deref(), Some("/healthz"));
    assert!(app.engine.calls().is_empty());
}

#[tokio::test]
async fn raw_yaml_body_with_query_dry_run() {
    let app = TestApp::new().await;
    for ct in ["application/yaml", "text/yaml", "application/x-yaml"] {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/blueprints/apply?dry_run=true")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("content-type", ct)
            .body(Body::from(EXAMPLE))
            .unwrap();
        let r = app.send(req).await;
        assert_eq!(r.status, StatusCode::OK, "{ct}: {}", r.text());
        assert_eq!(r.json()["dry_run"], true);
    }
    assert!(app.store.list_services().await.unwrap().is_empty());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/blueprints/apply")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/yaml")
        .body(Body::from(EXAMPLE))
        .unwrap();
    let r = app.send(req).await;
    assert_eq!(r.json()["dry_run"], false);
    assert_eq!(app.store.list_services().await.unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_entries_fail_the_whole_apply() {
    let app = TestApp::new().await;
    let cases = [
        // bad numInstances on the second service; the first entries are valid
        (
            "envVarGroups:\n  - name: g\n    envVars: [{key: A, value: b}]\nservices:\n  - {type: worker, name: ok, image: busybox}\n  - {type: worker, name: broken, image: busybox, numInstances: 0}\n",
            "broken",
        ),
        (
            "services:\n  - {type: web, name: w, envVars: [{key: DB, fromDatabase: {name: nope, property: connectionString}}]}\n",
            "nope",
        ),
        ("services:\n  - {type: web, name: w, envVars: [{fromGroup: missing}]}\n", "missing"),
        (
            "services:\n  - {type: web, name: w, envVars: [{key: X, fromService: {type: pserv, name: w2, envVarKey: Y}}]}\n",
            "w2",
        ),
        ("databases:\n  - {name: d, postgresMajorVersion: latest}\n", "database 'd'"),
        ("services:\n  - {type: cron, name: c, image: busybox}\n", "service 'c'"),
        (
            "services:\n  - {type: web, name: a, domains: [x.example.com]}\n  - {type: web, name: b, domains: [x.example.com]}\n",
            "x.example.com",
        ),
        ("services:\n  - {type: web, name: dup}\n  - {type: worker, name: dup}\n", "dup"),
        ("services:\n  - {type: web, name: Bad_Name}\n", "Bad_Name"),
        (
            "services:\n  - {type: web, name: w, envVars: [{key: X, fromService: {name: w, property: connectionString}}]}\n",
            "connectionString",
        ),
        ("services: {}\n", "services"),
    ];
    for (yaml, needle) in cases {
        let (status, res) = apply(&app, yaml, false).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{yaml}: {res}");
        let msg = res["error"]["message"].as_str().unwrap();
        assert!(msg.contains(needle), "{msg} should mention {needle}");
    }
    assert!(app.store.list_services().await.unwrap().is_empty());
    assert!(app.store.list_env_groups().await.unwrap().is_empty());
    assert!(app.store.list_datastores().await.unwrap().is_empty());
    assert!(app.engine.calls().is_empty());

    // name clashes with existing resources of the other kind → 409
    app.create_service(json!({"name": "taken"})).await;
    let (status, _) = apply(&app, "databases:\n  - {name: taken}\n", false).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // unparseable YAML
    let (status, _) = apply(&app, "services: [", false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn existing_services_keep_instances_and_domains_when_omitted() {
    let app = TestApp::new().await;
    let yaml = "services:\n  - {type: web, name: w, image: nginx}\n";
    apply(&app, yaml, false).await;
    app.patch("/api/v1/services/w", json!({"instances": 4, "custom_domains": ["w.example.com"]})).await;
    let (_, res) = apply(&app, yaml, false).await;
    assert_eq!(action_of(&res, "w")["action"], "unchanged", "{res}");
    let w = app.store.require_service("w").await.unwrap();
    assert_eq!((w.instances, w.custom_domains.clone()), (4, vec!["w.example.com".to_string()]));
    // but other settings are declarative: an omitted key resets to the default
    app.patch("/api/v1/services/w", json!({"health_check_path": "/h"})).await;
    let (_, res) = apply(&app, yaml, false).await;
    assert_eq!(action_of(&res, "w")["changes"], json!(["health_check_path: /h → (none)"]));
}

#[tokio::test]
async fn existing_services_keep_their_source_when_repo_and_image_are_omitted() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "w", "repo_url": "https://github.com/a/b", "branch": "dev", "deploy": false}))
        .await;
    let (status, res) = apply(&app, "services:\n  - {type: web, name: w}\n", false).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(action_of(&res, "w")["action"], "unchanged", "{res}");
    let w = app.store.require_service("w").await.unwrap();
    assert_eq!((w.repo_url.as_deref(), w.branch.as_str()), (Some("https://github.com/a/b"), "dev"));
    // an explicit repo replaces it (and resets the branch to the default)
    let (_, res) = apply(&app, "services:\n  - {type: web, name: w, repo: 'https://github.com/a/c'}\n", false).await;
    assert_eq!(
        action_of(&res, "w")["changes"],
        json!(["repo_url: https://github.com/a/b → https://github.com/a/c", "branch: dev → main"])
    );
    assert_eq!(res["deploys"].as_array().unwrap().len(), 1);
    assert_eq!(res["deploys"][0]["trigger"], "blueprint");
}

#[tokio::test]
async fn existing_datastores_are_never_changed() {
    let app = TestApp::new().await;
    apply(&app, "databases:\n  - {name: db, postgresMajorVersion: '15'}\n", false).await;
    let (status, res) = apply(&app, "databases:\n  - {name: db, postgresMajorVersion: '16'}\n", false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(action_of(&res, "db")["action"], "unchanged");
    assert!(res["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("not changed in place")));
    assert_eq!(app.store.require_datastore("db").await.unwrap().version, "15");
    // a redis blueprint entry can't take over a postgres datastore
    let (status, _) = apply(&app, "services:\n  - {type: redis, name: db}\n", false).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn env_var_key_copies_from_existing_services() {
    let app = TestApp::new().await;
    app.create_service(json!({"name": "auth", "env": [{"key": "TOKEN", "value": "t0k"}]})).await;
    let yaml = "services:\n  - {type: worker, name: w, envVars: [{key: T, fromService: {type: web, name: auth, envVarKey: TOKEN}}, {key: U, fromService: {name: auth, envVarKey: MISSING}}]}\n";
    let (status, res) = apply(&app, yaml, false).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let env = env_of(&app, "w").await;
    assert_eq!(get(&env, "T"), Some("t0k"));
    assert_eq!(get(&env, "U"), None);
    assert!(res["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("MISSING")));
}
