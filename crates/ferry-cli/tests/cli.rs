//! HTTP-level tests: the real `ferry` binary against a fake Ferry API.

mod common;

use std::collections::BTreeSet;
use std::io::Read;

use chrono::Utc;
use common::{Fake, Reply, TOKEN, ferry, ferry_in};
use ferry_core::dto::{DatastoreView, InstanceStatus, RuntimeStatus, ServiceView};
use ferry_core::{
    Datastore, DatastoreKind, DatastoreStatus, Deploy, DeploySource, DeployStatus, DeployTrigger, JobRun, JobStatus,
    JobTrigger, Service, ServiceState, ServiceType,
};
use serde_json::{Value, json};

fn service_view(name: &str, t: ServiceType) -> ServiceView {
    let mut service = Service::new(name, t);
    service.id = format!("srv-{name}");
    let public = service.is_public_http();
    ServiceView {
        deploy_hook_path: format!("/hooks/deploy/{}?key=secret", service.id),
        url: public.then(|| format!("http://{name}.localhost:8080")),
        hosts: if public { vec![format!("{name}.localhost")] } else { vec![] },
        internal_host: name.to_string(),
        internal_port: Some(10000),
        env_groups: vec![],
        latest_deploy: None,
        state: ServiceState::Live,
        service,
    }
}

fn deploy(id: &str, service_id: &str, status: DeployStatus) -> Deploy {
    let mut d = Deploy::new(service_id, DeployTrigger::Manual, DeploySource::Image { image: "nginx:alpine".into() });
    d.id = id.to_string();
    d.status = status;
    d
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap()
}

/// An SSE `log` event for a line.
fn log_event(stream: &str, instance: Option<&str>, line: &str) -> String {
    let mut v = json!({ "ts": "2026-01-10T12:00:05Z", "stream": stream, "line": line });
    if let Some(i) = instance {
        v["instance"] = json!(i);
    }
    format!("event: log\ndata: {v}\n\n")
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn services_table_and_raw_json() {
    let (fake, url) = Fake::start().await;
    let mut web = service_view("web", ServiceType::WebService);
    web.latest_deploy = Some(deploy("dep-1", "srv-web", DeployStatus::Live));
    let mut worker = service_view("jobs", ServiceType::BackgroundWorker);
    worker.state = ServiceState::Failed;
    let list = json!([to_json(&web), to_json(&worker)]);
    fake.on("GET", "/api/v1/services", Reply::ok(list.clone()));
    let h = home();

    let out = ferry(&url, h.path(), &["services"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", out.stdout);
    assert!(lines[0].starts_with("NAME   TYPE     STATE    INSTANCES   URL"), "{}", lines[0]);
    assert!(
        lines[1].starts_with("web    web      live     1           http://web.localhost:8080   live · "),
        "{}",
        lines[1]
    );
    assert!(lines[2].starts_with("jobs   worker   failed   1"), "{}", lines[2]);
    assert!(!out.stdout.contains('\x1b'), "no colors when piped");

    let out = ferry(&url, h.path(), &["ls", "--json"]).await;
    assert_eq!(out.code, 0);
    let printed: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(printed, list);

    let req = &fake.find("GET", "/api/v1/services")[0];
    assert_eq!(req.header("authorization").as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn api_errors_are_printed_with_their_code() {
    let (fake, url) = Fake::start().await;
    fake.on("GET", "/api/v1/services/nope", Reply::error(404, "not_found", "service 'nope' not found"));
    let h = home();
    let out = ferry(&url, h.path(), &["show", "nope"]).await;
    assert_eq!(out.code, 1);
    assert_eq!(out.stderr.trim(), "error: service 'nope' not found (not_found)");
    assert!(out.stdout.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_token_gets_a_login_hint() {
    let (_fake, url) = Fake::start().await;
    let h = home();
    let out = ferry_in(None, h.path(), Some(&url), Some("wrong"), &["info"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("error: invalid or missing API token (unauthorized)"), "{}", out.stderr);
    assert!(out.stderr.contains("hint: check the server URL and token with 'ferry login"), "{}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_server_is_friendly() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let h = home();
    let server = format!("http://127.0.0.1:{port}");
    let out = ferry(&server, h.path(), &["services"]).await;
    assert_eq!(out.code, 1);
    assert_eq!(out.stderr.trim(), format!("error: cannot reach Ferry server at {server} — is ferryd running?"));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_config_asks_to_log_in() {
    let h = home();
    let out = ferry_in(None, h.path(), None, None, &["services"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("run 'ferry login"), "{}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_verifies_and_saves_config_then_commands_use_it() {
    let (fake, url) = Fake::start().await;
    let info = json!({
        "version": "0.1.0", "base_domain": "localhost", "proxy_url": "http://localhost:8080",
        "tls_enabled": false, "dashboard_url": "http://ferry.localhost:8080",
        "github_webhook_enabled": false, "docker_version": "27.3.1"
    });
    fake.on("GET", "/api/v1/info", Reply::ok(info));
    let h = home();

    // Wrong token: nothing saved.
    let out = ferry_in(None, h.path(), None, None, &["login", "--server", &url, "--token", "nope"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    let path = h.path().join("xdg").join("ferry").join("config.json");
    assert!(!path.exists());

    let out = ferry_in(None, h.path(), None, None, &["login", "--server", &format!("{url}/"), "--token", TOKEN]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.contains(&format!("Logged in to {url} (Ferry 0.1.0)")), "{}", out.stdout);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved, json!({ "server": url, "token": TOKEN }));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    // No env, no flags: the saved config is used.
    let out = ferry_in(None, h.path(), None, None, &["info"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.contains("Docker:"), "{}", out.stdout);
    assert!(out.stdout.contains("27.3.1"));
    // A flag overrides the saved server.
    let out = ferry_in(None, h.path(), None, None, &["info", "--server", "http://127.0.0.1:9"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("cannot reach Ferry server at http://127.0.0.1:9"), "{}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_follow_streams_logs_and_reports_live() {
    let (fake, url) = Fake::start().await;
    let queued = deploy("dep-1", "srv-web", DeployStatus::Queued);
    fake.on("POST", "/api/v1/services/web/deploys", Reply::json(202, to_json(&queued)));
    let first = log_event("system", None, "==> Cloning from https://github.com/a/b (branch main)");
    let second = log_event("stdout", Some("a1b2c3"), "hello from the app");
    let all = format!("{first}: keep-alive\n\n{second}event: end\n\n");
    // Split at awkward places: mid-field, between \r and \n equivalents, mid-UTF-8.
    let bytes = all.as_bytes();
    let cuts = [7, 30, first.len() + 3, first.len() + 20, bytes.len() - 5];
    let mut chunks = Vec::new();
    let mut prev = 0;
    for c in cuts {
        chunks.push(String::from_utf8_lossy(&bytes[prev..c]).into_owned());
        prev = c;
    }
    chunks.push(String::from_utf8_lossy(&bytes[prev..]).into_owned());
    let chunk_refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
    fake.on("GET", "/api/v1/deploys/dep-1/logs", Reply::sse(&chunk_refs));
    fake.on("GET", "/api/v1/deploys/dep-1", Reply::ok(to_json(&deploy("dep-1", "srv-web", DeployStatus::Live))));
    fake.on("GET", "/api/v1/services/srv-web", Reply::ok(to_json(&service_view("web", ServiceType::WebService))));
    let h = home();

    let out = ferry(&url, h.path(), &["deploy", "web", "--commit", "abc123", "--clear-cache", "--follow"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", out.stdout);
    assert!(lines[0].ends_with(" ==> Cloning from https://github.com/a/b (branch main)"), "{}", lines[0]);
    assert!(lines[1].ends_with(" [a1b2c3] hello from the app"), "{}", lines[1]);
    assert_eq!(lines[0].split(' ').next().unwrap().len(), 8, "HH:MM:SS prefix");
    assert_eq!(lines[2], "Deploy dep-1 is live at http://web.localhost:8080");

    let post = &fake.find("POST", "/api/v1/services/web/deploys")[0];
    assert_eq!(post.json(), json!({ "commit": "abc123", "clear_cache": true }));
    let logs = &fake.find("GET", "/api/v1/deploys/dep-1/logs")[0];
    assert_eq!(logs.query, "follow=true");
    assert_eq!(logs.header("accept").as_deref(), Some("text/event-stream"));
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_follow_failure_exits_1() {
    let (fake, url) = Fake::start().await;
    fake.on(
        "POST",
        "/api/v1/services/web/deploys",
        Reply::json(202, to_json(&deploy("dep-2", "srv-web", DeployStatus::Queued))),
    );
    fake.on(
        "GET",
        "/api/v1/deploys/dep-2/logs",
        Reply::sse(&[&log_event("stderr", None, "npm ERR! missing script: build"), "event: end\n\n"]),
    );
    let mut failed = deploy("dep-2", "srv-web", DeployStatus::BuildFailed);
    failed.error = Some("build command exited with status 1".into());
    fake.on("GET", "/api/v1/deploys/dep-2", Reply::ok(to_json(&failed)));
    let h = home();
    let out = ferry(&url, h.path(), &["deploy", "web", "-f"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(out.stdout.contains("npm ERR! missing script: build"));
    assert!(
        out.stdout.contains("Deploy dep-2 failed (build_failed): build command exited with status 1"),
        "{}",
        out.stdout
    );
    assert!(!out.stderr.contains("error:"), "a failed deploy is reported once: {}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_without_follow_prints_id_and_hint() {
    let (fake, url) = Fake::start().await;
    fake.on(
        "POST",
        "/api/v1/services/web/deploys",
        Reply::json(202, to_json(&deploy("dep-3", "srv-web", DeployStatus::Queued))),
    );
    let h = home();
    let out = ferry(&url, h.path(), &["deploy", "web"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "Deploy dep-3 queued (manual)\nFollow it with: ferry logs --deploy dep-3 -f\n");
    assert_eq!(
        fake.find("POST", "/api/v1/services/web/deploys")[0].json(),
        json!({ "commit": null, "clear_cache": false })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn follow_reconnects_without_duplicating_lines() {
    let (fake, url) = Fake::start().await;
    let a = log_event("stdout", None, "line one");
    let b = log_event("stdout", None, "line two");
    let c = log_event("stdout", None, "line three");
    // First connection drops after two lines (no `end`); the reconnect replays
    // the stored log from the start, then continues.
    fake.on("GET", "/api/v1/deploys/dep-4/logs", Reply::sse(&[&a, &b]));
    fake.on("GET", "/api/v1/deploys/dep-4/logs", Reply::sse(&[&a, &b, &c, "event: end\n\n"]));
    fake.on("GET", "/api/v1/deploys/dep-4", Reply::ok(to_json(&deploy("dep-4", "srv-web", DeployStatus::Building))));
    fake.on("GET", "/api/v1/deploys/dep-4", Reply::ok(to_json(&deploy("dep-4", "srv-web", DeployStatus::Live))));
    fake.on("GET", "/api/v1/services/srv-web", Reply::ok(to_json(&service_view("web", ServiceType::WebService))));
    let h = home();
    let out = ferry(&url, h.path(), &["logs", "--deploy", "dep-4", "-f"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let texts: Vec<&str> = out.stdout.lines().map(|l| l.split_once(' ').unwrap().1).collect();
    assert_eq!(texts, vec!["line one", "line two", "line three", "dep-4 is live at http://web.localhost:8080"]);
    assert_eq!(fake.find("GET", "/api/v1/deploys/dep-4/logs").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn env_set_and_unset_patch_with_restart_flag() {
    let (fake, url) = Fake::start().await;
    let vars = json!([{ "key": "A", "value": "1" }, { "key": "URL", "value": "x=y" }]);
    fake.on("PATCH", "/api/v1/services/web/env", Reply::ok(vars.clone()));
    fake.on("GET", "/api/v1/services/web/env", Reply::ok(vars));
    let h = home();

    let out = ferry(&url, h.path(), &["env", "set", "web", "A=1", "URL=x=y"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "A=1\nURL=x=y\n");
    let out = ferry(&url, h.path(), &["env", "unset", "web", "OLD", "--no-restart"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let out = ferry(&url, h.path(), &["env", "web"]).await;
    assert_eq!(out.stdout, "A=1\nURL=x=y\n");

    let patches = fake.find("PATCH", "/api/v1/services/web/env");
    assert_eq!(patches[0].query, "restart=true");
    assert_eq!(
        patches[0].json(),
        json!({ "set": [{ "key": "A", "value": "1" }, { "key": "URL", "value": "x=y" }], "unset": [] })
    );
    assert_eq!(patches[1].query, "restart=false");
    assert_eq!(patches[1].json(), json!({ "set": [], "unset": ["OLD"] }));
}

fn tar_names(gz: &[u8]) -> BTreeSet<String> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(gz));
    ar.entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().trim_end_matches('/').to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn up_creates_the_service_and_uploads_a_tarball() {
    let (fake, url) = Fake::start().await;
    let project = tempfile::tempdir().unwrap();
    let dir = project.path().join("My_Site");
    std::fs::create_dir_all(dir.join("node_modules/x")).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join("index.html"), "<h1>hi</h1>").unwrap();
    std::fs::write(dir.join(".env.example"), "A=1").unwrap();
    std::fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(dir.join(".ferryignore"), "draft.html\n").unwrap();
    std::fs::write(dir.join("debug.log"), "x").unwrap();
    std::fs::write(dir.join("draft.html"), "x").unwrap();
    std::fs::write(dir.join("node_modules/x/i.js"), "x").unwrap();
    std::fs::write(dir.join(".git/HEAD"), "ref").unwrap();

    fake.on("GET", "/api/v1/services/my-site", Reply::error(404, "not_found", "service 'my-site' not found"));
    let mut created = service_view("my-site", ServiceType::StaticSite);
    created.state = ServiceState::NotDeployed;
    fake.on("POST", "/api/v1/services", Reply::json(201, to_json(&created)));
    let mut d = deploy("dep-9", "srv-my-site", DeployStatus::Queued);
    d.trigger = DeployTrigger::Upload;
    fake.on("POST", "/api/v1/services/srv-my-site/deploys/upload", Reply::json(202, to_json(&d)));
    let h = home();

    let out = ferry_in(Some(&dir), h.path(), Some(&url), Some(TOKEN), &["up", "--type", "static", "-e", "K=V"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stderr.contains("Packed 4 file(s)"), "{}", out.stderr);
    assert!(out.stderr.contains("Created static site 'my-site' (srv-my-site)"), "{}", out.stderr);
    assert!(out.stdout.contains("Deploy dep-9 queued (upload)"), "{}", out.stdout);
    assert!(out.stdout.contains("ferry logs --deploy dep-9 -f"));

    let create = fake.find("POST", "/api/v1/services")[0].json();
    assert_eq!(create["name"], "my-site");
    assert_eq!(create["type"], "static_site");
    assert_eq!(create["deploy"], false);
    assert_eq!(create["env"], json!([{ "key": "K", "value": "V" }]));

    let upload = &fake.find("POST", "/api/v1/services/srv-my-site/deploys/upload")[0];
    assert_eq!(upload.header("content-type").as_deref(), Some("application/gzip"));
    assert_eq!(upload.header("content-length"), Some(upload.body.len().to_string()));
    assert_eq!(upload.query, "");
    let names = tar_names(&upload.body);
    let expected: BTreeSet<String> =
        [".env.example", ".ferryignore", ".gitignore", "index.html"].iter().map(|s| s.to_string()).collect();
    assert_eq!(names, expected);
    // Every file is in the archive with its content.
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&upload.body[..]));
    let mut html = String::new();
    for e in ar.entries().unwrap() {
        let mut e = e.unwrap();
        if e.path().unwrap().to_string_lossy() == "index.html" {
            e.read_to_string(&mut html).unwrap();
        }
    }
    assert_eq!(html, "<h1>hi</h1>");
}

#[tokio::test(flavor = "multi_thread")]
async fn up_existing_service_skips_creation() {
    let (fake, url) = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("app.py"), "print(1)").unwrap();
    fake.on("GET", "/api/v1/services/api-app", Reply::ok(to_json(&service_view("api-app", ServiceType::WebService))));
    let mut d = deploy("dep-10", "srv-api-app", DeployStatus::Queued);
    d.trigger = DeployTrigger::Upload;
    fake.on("POST", "/api/v1/services/srv-api-app/deploys/upload", Reply::json(202, to_json(&d)));
    let h = home();
    let dir_arg = dir.path().to_str().unwrap();
    let out =
        ferry(&url, h.path(), &["up", "api-app", "--dir", dir_arg, "--port", "8000", "--clear-cache", "--json"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stderr.contains("creation flags are ignored"), "{}", out.stderr);
    assert!(fake.find("POST", "/api/v1/services").is_empty());
    let upload = &fake.find("POST", "/api/v1/services/srv-api-app/deploys/upload")[0];
    assert_eq!(upload.query, "clear_cache=true");
    // --json prints the raw deploy.
    let printed: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(printed["id"], "dep-10");
}

#[tokio::test(flavor = "multi_thread")]
async fn up_refuses_a_reserved_directory_name() {
    let (_fake, url) = Fake::start().await;
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("api");
    std::fs::create_dir(&dir).unwrap();
    let h = home();
    let out = ferry_in(Some(&dir), h.path(), Some(&url), Some(TOKEN), &["up"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("reserved") && out.stderr.contains("ferry up api-app"), "{}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_requires_confirmation_without_a_tty() {
    let (fake, url) = Fake::start().await;
    fake.on("DELETE", "/api/v1/services/web", Reply::no_content());
    let h = home();
    let out = ferry(&url, h.path(), &["delete", "web"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("refusing to delete service 'web'"), "{}", out.stderr);
    assert!(out.stderr.contains("--yes"));
    assert!(fake.find("DELETE", "/api/v1/services/web").is_empty());

    let out = ferry(&url, h.path(), &["delete", "web", "--yes"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "Deleted service 'web'\n");
    assert_eq!(fake.find("DELETE", "/api/v1/services/web").len(), 1);

    // Same rule for datastores and env groups.
    let out = ferry(&url, h.path(), &["db", "rm", "main"]).await;
    assert_eq!(out.code, 1);
    let out = ferry(&url, h.path(), &["env-group", "rm", "shared"]).await;
    assert_eq!(out.code, 1);
    assert!(
        fake.requests().iter().all(|r| r.path != "/api/v1/datastores/main" && r.path != "/api/v1/env-groups/shared")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_follow_streams_job_and_fails_with_its_status() {
    let (fake, url) = Fake::start().await;
    let job = JobRun::new("srv-api", JobTrigger::Manual, Some("echo 'hello world'".into()));
    let mut job = job;
    job.id = "job-1".into();
    fake.on("POST", "/api/v1/services/api/jobs", Reply::json(202, to_json(&job)));
    fake.on(
        "GET",
        "/api/v1/jobs/job-1/logs",
        Reply::sse(&[&log_event("stdout", None, "hello world"), "event: end\n\n"]),
    );
    job.status = JobStatus::Failed;
    job.exit_code = Some(3);
    fake.on("GET", "/api/v1/jobs/job-1", Reply::ok(to_json(&job)));
    let h = home();
    let out = ferry(&url, h.path(), &["run", "api", "--follow", "--", "echo", "hello world"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(out.stdout.contains(" hello world\n"), "{}", out.stdout);
    assert!(out.stdout.contains("Job job-1 failed (exit code 3)"), "{}", out.stdout);
    let post = &fake.find("POST", "/api/v1/services/api/jobs")[0];
    assert_eq!(post.json(), json!({ "command": "echo 'hello world'" }));

    // No command → null (cron jobs run their start command).
    let out = ferry(&url, h.path(), &["run", "api"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.contains("ferry logs --job job-1 -f"));
    assert_eq!(fake.find("POST", "/api/v1/services/api/jobs")[1].json(), json!({ "command": null }));
}

#[tokio::test(flavor = "multi_thread")]
async fn blueprint_apply_sends_the_yaml_and_groups_actions() {
    let (fake, url) = Fake::start().await;
    let result = json!({
        "dry_run": true,
        "actions": [
            { "resource": "datastore", "name": "app-db", "action": "create", "changes": [] },
            { "resource": "service", "name": "hello", "action": "update", "changes": ["instances: 1 → 2"] },
        ],
        "deploys": [],
        "warnings": ["services[0].plan is ignored"],
    });
    fake.on("POST", "/api/v1/blueprints/apply", Reply::ok(result));
    let dir = tempfile::tempdir().unwrap();
    let yaml = "services:\n  - type: web\n    name: hello\n";
    std::fs::write(dir.path().join("render.yaml"), yaml).unwrap();
    let h = home();

    let out = ferry_in(Some(dir.path()), h.path(), Some(&url), Some(TOKEN), &["blueprint", "apply", "--dry-run"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.starts_with("Blueprint render.yaml — dry run, nothing was changed"), "{}", out.stdout);
    assert!(out.stdout.contains("Would create:\n  + datastore app-db\n"), "{}", out.stdout);
    assert!(out.stdout.contains("Would update:\n  ~ service hello\n      - instances: 1 → 2\n"));
    assert!(out.stdout.contains("Warnings:\n  ! services[0].plan is ignored"));
    let req = &fake.find("POST", "/api/v1/blueprints/apply")[0];
    assert_eq!(req.json(), json!({ "yaml": yaml, "dry_run": true }));
    assert_eq!(req.header("content-type").as_deref(), Some("application/json"));

    // No blueprint in the directory.
    let empty = tempfile::tempdir().unwrap();
    let out = ferry_in(Some(empty.path()), h.path(), Some(&url), Some(TOKEN), &["blueprint", "apply"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no blueprint found"), "{}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn names_and_ids_are_url_encoded() {
    let (fake, url) = Fake::start().await;
    let h = home();
    let out = ferry(&url, h.path(), &["show", "a b/c?d"]).await;
    assert_eq!(out.code, 1);
    assert_eq!(fake.requests()[0].path, "/api/v1/services/a%20b%2Fc%3Fd");
    let out = ferry(&url, h.path(), &["domains", "rm", "web", "app.example.com"]).await;
    assert_eq!(out.code, 1);
    assert_eq!(fake.requests()[1].path, "/api/v1/services/web/domains/app.example.com");
    assert_eq!(fake.requests()[1].method, "DELETE");
    let out = ferry(&url, h.path(), &["show", ".."]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("invalid name or id '..'"), "{}", out.stderr);
    assert_eq!(fake.requests().len(), 2, "no request for '..'");
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_logs_with_tail() {
    let (fake, url) = Fake::start().await;
    fake.on(
        "GET",
        "/api/v1/services/web/logs",
        Reply::sse(&[
            &log_event("stdout", Some("aaaaaa"), "GET / 200"),
            ": keep-alive\n\n",
            &log_event("stderr", Some("bbbbbb"), "warning: slow"),
            "event: end\ndata:\n\n",
        ]),
    );
    let h = home();
    let out = ferry(&url, h.path(), &["logs", "web", "--tail", "5"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with(" [aaaaaa] GET / 200"));
    assert!(lines[1].ends_with(" [bbbbbb] warning: slow"));
    assert_eq!(fake.find("GET", "/api/v1/services/web/logs")[0].query, "follow=false&tail=5");

    // --json prints each log line as raw JSON.
    let out = ferry(&url, h.path(), &["logs", "web", "--json"]).await;
    let first: Value = serde_json::from_str(out.stdout.lines().next().unwrap()).unwrap();
    assert_eq!(first["line"], "GET / 200");
}

#[tokio::test(flavor = "multi_thread")]
async fn status_shows_instances_with_cpu_and_memory() {
    let (fake, url) = Fake::start().await;
    let inst = |name: &str, state: &str, cpu: Option<f64>, mem: Option<u64>| InstanceStatus {
        container_id: "c".into(),
        name: name.into(),
        deploy_id: Some("dep-0123456789abcdefghij".into()),
        state: state.into(),
        host_port: Some(54321),
        started_at: Some((Utc::now() - chrono::Duration::minutes(3)).to_rfc3339()),
        restart_count: Some(0),
        cpu_percent: cpu,
        memory_bytes: mem,
        memory_limit_bytes: Some(512 * 1024 * 1024),
    };
    let status = RuntimeStatus {
        service_id: "srv-web".into(),
        state: ServiceState::Degraded,
        desired_instances: 2,
        instances: vec![
            inst("/ferry-web-cdefghij-a1b2c3", "running", Some(12.5), Some(12 * 1024 * 1024)),
            inst("/ferry-web-cdefghij-d4e5f6", "exited", None, None),
        ],
    };
    fake.on("GET", "/api/v1/services/web/status", Reply::ok(to_json(&status)));
    let h = home();
    let out = ferry(&url, h.path(), &["status", "web"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines[0], "web: degraded — 1/2 instance(s) running");
    assert!(lines[1].starts_with("INSTANCE   DEPLOY     STATE     PORT    CPU     MEMORY"), "{}", lines[1]);
    assert!(
        lines[2]
            .starts_with("a1b2c3     cdefghij   running   54321   12.5%   12.0 MiB / 512.0 MiB   0          3m ago"),
        "{}",
        lines[2]
    );
    assert!(lines[3].starts_with("d4e5f6     cdefghij   exited    54321   -       -"), "{}", lines[3]);
}

#[tokio::test(flavor = "multi_thread")]
async fn db_show_prints_internal_and_external_urls() {
    let (fake, url) = Fake::start().await;
    let mut ds = Datastore::new("app-db", DatastoreKind::Postgres);
    ds.password = "pw".into();
    ds.status = DatastoreStatus::Available;
    ds.host_port = Some(15432);
    let view = DatastoreView {
        internal_host: "app-db".into(),
        internal_port: 5432,
        internal_url: ds.internal_url(),
        external_url: ds.external_url("127.0.0.1"),
        datastore: ds,
    };
    fake.on("GET", "/api/v1/datastores/app-db", Reply::ok(to_json(&view)));
    let h = home();
    let out = ferry(&url, h.path(), &["db", "show", "app-db"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.contains("Internal URL:   postgresql://app_db:pw@app-db:5432/app_db"), "{}", out.stdout);
    assert!(out.stdout.contains("External URL:   postgresql://app_db:pw@127.0.0.1:15432/app_db"), "{}", out.stdout);
    assert!(out.stdout.contains("${{datastore.app-db.connectionString}}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn create_follow_streams_the_first_deploy() {
    let (fake, url) = Fake::start().await;
    let mut view = service_view("hello", ServiceType::WebService);
    view.service.image = Some("nginx:alpine".into());
    view.latest_deploy = Some(deploy("dep-5", "srv-hello", DeployStatus::Queued));
    fake.on("POST", "/api/v1/services", Reply::json(201, to_json(&view)));
    fake.on(
        "GET",
        "/api/v1/deploys/dep-5/logs",
        Reply::sse(&[&log_event("system", None, "==> Pulling nginx:alpine"), "event: end\n\n"]),
    );
    fake.on("GET", "/api/v1/deploys/dep-5", Reply::ok(to_json(&deploy("dep-5", "srv-hello", DeployStatus::Live))));
    fake.on("GET", "/api/v1/services/srv-hello", Reply::ok(to_json(&view)));
    let h = home();
    let out =
        ferry(&url, h.path(), &["create", "hello", "--image", "nginx:alpine", "--port", "80", "--env", "A=1", "-f"])
            .await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(
        out.stdout.starts_with("Created web service 'hello' (srv-hello)\nURL: http://hello.localhost:8080\n"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("==> Pulling nginx:alpine"));
    assert!(out.stdout.ends_with("Deploy dep-5 is live at http://hello.localhost:8080\n"), "{}", out.stdout);
    let body = fake.find("POST", "/api/v1/services")[0].json();
    assert_eq!(body["name"], "hello");
    assert_eq!(body["image"], "nginx:alpine");
    assert_eq!(body["port"], 80);
    assert_eq!(body["env"], json!([{ "key": "A", "value": "1" }]));
    assert!(body["type"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn update_scale_and_suspend() {
    let (fake, url) = Fake::start().await;
    let view = service_view("web", ServiceType::WebService);
    fake.on("PATCH", "/api/v1/services/web", Reply::ok(to_json(&view)));
    let mut scaled = view.clone();
    scaled.service.instances = 3;
    fake.on("POST", "/api/v1/services/web/scale", Reply::ok(to_json(&scaled)));
    let mut suspended = view.clone();
    suspended.state = ServiceState::Suspended;
    fake.on("POST", "/api/v1/services/web/suspend", Reply::ok(to_json(&suspended)));
    let h = home();

    let out = ferry(&url, h.path(), &["update", "web"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("nothing to update"));

    let out = ferry(&url, h.path(), &["update", "web", "--start-cmd", "", "--no-auto-deploy"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.contains("Updated 'web'"));
    assert!(out.stdout.contains("ferry deploy web"));
    let body = fake.find("PATCH", "/api/v1/services/web")[0].json();
    assert_eq!(body["start_command"], "");
    assert_eq!(body["auto_deploy"], false);
    assert!(body["instances"].is_null());

    let out = ferry(&url, h.path(), &["scale", "web", "3"]).await;
    assert_eq!(out.stdout, "Scaled 'web' to 3 instance(s)\n");
    assert_eq!(fake.find("POST", "/api/v1/services/web/scale")[0].json(), json!({ "instances": 3 }));

    let out = ferry(&url, h.path(), &["suspend", "web"]).await;
    assert_eq!(out.stdout, "Suspended 'web' (state: suspended)\n");
    assert!(fake.find("POST", "/api/v1/services/web/suspend")[0].body.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn open_prints_the_url_or_explains() {
    let (fake, url) = Fake::start().await;
    fake.on(
        "GET",
        "/api/v1/services/internal",
        Reply::ok(to_json(&service_view("internal", ServiceType::PrivateService))),
    );
    let h = home();
    let out = ferry(&url, h.path(), &["open", "internal"]).await;
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("private service with no public URL") && out.stderr.contains("internal:10000"),
        "{}",
        out.stderr
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_while_following_exits_130_with_a_resume_hint() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let (fake, url) = Fake::start().await;
    fake.on("GET", "/api/v1/deploys/dep-7/logs", Reply::sse_open(&[&log_event("stdout", None, "building…")]));
    let h = home();
    let mut child =
        common::command(None, h.path(), Some(&url), Some(TOKEN), &["logs", "--deploy", "dep-7", "-f"]).spawn().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(20), stdout.read_line(&mut first)).await.unwrap().unwrap();
    assert!(first.ends_with(" building…\n"), "{first}");

    let pid = child.id().unwrap().to_string();
    let status = std::process::Command::new("kill").args(["-INT", &pid]).status().unwrap();
    assert!(status.success());
    let exit = tokio::time::timeout(std::time::Duration::from_secs(20), child.wait()).await.unwrap().unwrap();
    assert_eq!(exit.code(), Some(130));
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).await.unwrap();
    assert!(stderr.contains("Resume with: ferry logs --deploy dep-7 -f"), "{stderr}");
}
