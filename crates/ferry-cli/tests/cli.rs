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
    // The same server spelled differently still uses the saved token.
    let out = ferry_in(None, h.path(), None, None, &["info", "--server", &format!("{url}/")]).await;
    assert_eq!(out.code, 0, "{out:?}");
    // A flag overrides the saved server (with its own token).
    let out = ferry_in(None, h.path(), None, None, &["info", "--server", "http://127.0.0.1:9", "--token", "t"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("cannot reach Ferry server at http://127.0.0.1:9"), "{}", out.stderr);

    // ...but the token saved for this server is never sent to another one.
    let (other, other_url) = Fake::start().await;
    let out = ferry_in(None, h.path(), None, None, &["info", "--server", &other_url]).await;
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains(&format!("no token for {other_url}"))
            && out.stderr.contains(&format!("saved login is for {url}")),
        "{}",
        out.stderr
    );
    let out = ferry_in(Some(h.path()), h.path(), Some(&other_url), None, &["services"]).await;
    assert_eq!(out.code, 1);
    let out = ferry_in(None, h.path(), None, None, &["login", "--server", &other_url]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains(&format!("missing token for {other_url}")), "{}", out.stderr);
    assert!(other.requests().is_empty(), "the saved token leaked: {:?}", other.requests());
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved, json!({ "server": url, "token": TOKEN }), "a failed login keeps the saved config");
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
    let web = to_json(&service_view("web", ServiceType::WebService));
    fake.on("GET", "/api/v1/services/web", Reply::ok(web.clone()));
    fake.on("GET", "/api/v1/services/srv-web", Reply::ok(web));
    fake.on("GET", "/api/v1/services/srv-web/env", Reply::ok(json!([{ "key": "OLD", "value": "x" }])));
    let vars = json!([{ "key": "A", "value": "1" }, { "key": "URL", "value": "x=y" }]);
    fake.on("PATCH", "/api/v1/services/srv-web/env", Reply::ok(vars.clone()));
    fake.on("GET", "/api/v1/services/web/env", Reply::ok(vars));
    let h = home();

    let out = ferry(&url, h.path(), &["env", "set", "web", "A=1", "URL=x=y"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "A=1\nURL=x=y\n");
    assert!(out.stderr.contains("'web' isn't live, so nothing restarts"), "{}", out.stderr);
    let out = ferry(&url, h.path(), &["env", "unset", "web", "OLD", "--no-restart"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(
        out.stderr.contains("Saved without restarting: the next deploy of 'web' uses the new environment."),
        "{}",
        out.stderr
    );
    let out = ferry(&url, h.path(), &["env", "web"]).await;
    assert_eq!(out.stdout, "A=1\nURL=x=y\n");

    let patches = fake.find("PATCH", "/api/v1/services/srv-web/env");
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
    // A virtualenv and a link leaving the directory: the server would reject
    // the whole upload because of such links.
    #[cfg(unix)]
    {
        std::fs::create_dir_all(dir.join("venv/bin")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/python3", dir.join("venv/bin/python")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/python3", dir.join("python")).unwrap();
    }

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
    #[cfg(unix)]
    assert!(
        out.stderr.contains("warning: not uploading 1 symlink(s) pointing outside")
            && out.stderr.contains("python -> /usr/bin/python3"),
        "{}",
        out.stderr
    );
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
async fn up_existing_service_applies_flags_then_uploads() {
    let (fake, url) = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("app.py"), "print(1)").unwrap();
    let view = service_view("api-app", ServiceType::WebService);
    fake.on("GET", "/api/v1/services/api-app", Reply::ok(to_json(&view)));
    let mut updated = view.clone();
    updated.service.start_command = Some("python -m http.server $PORT".into());
    fake.on("PATCH", "/api/v1/services/srv-api-app", Reply::ok(to_json(&updated)));
    fake.on("PATCH", "/api/v1/services/srv-api-app/env", Reply::ok(json!([{ "key": "K", "value": "V" }])));
    let mut linked = updated.clone();
    linked.env_groups = vec!["shared".into()];
    fake.on("POST", "/api/v1/services/srv-api-app/env-groups", Reply::ok(to_json(&linked)));
    let mut d = deploy("dep-10", "srv-api-app", DeployStatus::Queued);
    d.trigger = DeployTrigger::Upload;
    fake.on("POST", "/api/v1/services/srv-api-app/deploys/upload", Reply::json(202, to_json(&d)));
    let h = home();
    let dir_arg = dir.path().to_str().unwrap();
    let out = ferry(
        &url,
        h.path(),
        &[
            "up",
            "api-app",
            "--dir",
            dir_arg,
            "--start-cmd",
            "python -m http.server $PORT",
            "--port",
            "8000",
            "-e",
            "K=V",
            "--env-group",
            "shared",
            "--type",
            "web",
            "--clear-cache",
            "--json",
        ],
    )
    .await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(!out.stderr.contains("ignored"), "{}", out.stderr);
    assert!(out.stderr.contains("Updated 'api-app': start command; port; env K; env group shared"), "{}", out.stderr);
    assert!(fake.find("POST", "/api/v1/services").is_empty(), "no creation");
    // The settings are applied before the upload, like `ferry update` would.
    let patch = fake.find("PATCH", "/api/v1/services/srv-api-app")[0].json();
    assert_eq!(patch["start_command"], "python -m http.server $PORT");
    assert_eq!(patch["port"], 8000);
    assert!(patch["instances"].is_null() && patch["custom_domains"].is_null() && patch["repo_url"].is_null());
    let env = &fake.find("PATCH", "/api/v1/services/srv-api-app/env")[0];
    assert_eq!(env.query, "restart=false", "the upload deploys the new env");
    assert_eq!(env.json(), json!({ "set": [{ "key": "K", "value": "V" }], "unset": [] }));
    assert_eq!(fake.find("POST", "/api/v1/services/srv-api-app/env-groups")[0].json(), json!({ "group": "shared" }));
    let requests = fake.requests();
    let pos = |m: &str, p: &str| requests.iter().position(|r| r.method == m && r.path == p).unwrap();
    assert!(pos("PATCH", "/api/v1/services/srv-api-app") < pos("POST", "/api/v1/services/srv-api-app/deploys/upload"));
    let upload = &fake.find("POST", "/api/v1/services/srv-api-app/deploys/upload")[0];
    assert_eq!(upload.query, "clear_cache=true");
    // --json prints the raw deploy.
    let printed: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(printed["id"], "dep-10");

    // An existing service's type can't change: refused before anything is sent.
    let before = fake.requests().len();
    let out = ferry(&url, h.path(), &["up", "api-app", "--dir", dir_arg, "--type", "worker"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(out.stderr.contains("is a web service") && out.stderr.contains("--type worker"), "{}", out.stderr);
    let after: Vec<String> = fake.requests()[before..].iter().map(|r| format!("{} {}", r.method, r.path)).collect();
    assert_eq!(after, vec!["GET /api/v1/services/api-app"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn up_refuses_a_reserved_directory_name() {
    let (_fake, url) = Fake::start().await;
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("ferry");
    std::fs::create_dir(&dir).unwrap();
    let h = home();
    let out = ferry_in(Some(&dir), h.path(), Some(&url), Some(TOKEN), &["up"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("reserved") && out.stderr.contains("ferry up ferry-app"), "{}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_requires_confirmation_without_a_tty() {
    let (fake, url) = Fake::start().await;
    let web = to_json(&service_view("web", ServiceType::WebService));
    fake.on("GET", "/api/v1/services/web", Reply::ok(web.clone()));
    fake.on("GET", "/api/v1/services/srv-web", Reply::ok(web));
    fake.on("DELETE", "/api/v1/services/srv-web", Reply::no_content());
    let mut ds = Datastore::new("main", DatastoreKind::Postgres);
    ds.id = "dbs-main".into();
    let ds_view = DatastoreView {
        internal_host: "main".into(),
        internal_port: 5432,
        internal_url: ds.internal_url(),
        external_url: None,
        datastore: ds,
    };
    fake.on("GET", "/api/v1/datastores/main", Reply::ok(to_json(&ds_view)));
    fake.on("GET", "/api/v1/datastores/dbs-main", Reply::ok(to_json(&ds_view)));
    fake.on("DELETE", "/api/v1/datastores/dbs-main", Reply::no_content());
    let group = json!({
        "id": "evg-shared", "name": "shared", "created_at": "2026-01-10T12:00:00Z",
        "updated_at": "2026-01-10T12:00:00Z", "vars": [], "services": []
    });
    fake.on("GET", "/api/v1/env-groups/shared", Reply::ok(group.clone()));
    fake.on("GET", "/api/v1/env-groups/evg-shared", Reply::ok(group));
    fake.on("DELETE", "/api/v1/env-groups/evg-shared", Reply::no_content());
    let h = home();
    let out = ferry(&url, h.path(), &["delete", "web"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("refusing to delete service 'web'"), "{}", out.stderr);
    assert!(out.stderr.contains("--yes"));
    assert!(fake.requests().iter().all(|r| r.method != "DELETE"));

    let out = ferry(&url, h.path(), &["delete", "web", "--yes"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "Deleted service 'web'\n");
    // By id: the messages use the real name.
    let out = ferry(&url, h.path(), &["rm", "srv-web", "--yes"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "Deleted service 'web'\n");
    let out = ferry(&url, h.path(), &["rm", "srv-web", "--yes", "--json"]).await;
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap(), json!({ "deleted": "web", "id": "srv-web" }));
    assert_eq!(fake.find("DELETE", "/api/v1/services/srv-web").len(), 3, "deleted by id");

    // Same rules for datastores and env groups.
    let out = ferry(&url, h.path(), &["db", "rm", "main"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("refusing to delete datastore 'main'"), "{}", out.stderr);
    let out = ferry(&url, h.path(), &["env-group", "rm", "shared"]).await;
    assert_eq!(out.code, 1);
    assert!(fake.find("DELETE", "/api/v1/datastores/dbs-main").is_empty());
    assert!(fake.find("DELETE", "/api/v1/env-groups/evg-shared").is_empty());
    let out = ferry(&url, h.path(), &["db", "rm", "dbs-main", "--yes"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, "Deleted datastore 'main'\n"), "{out:?}");
    let out = ferry(&url, h.path(), &["env-group", "rm", "evg-shared", "--yes"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, "Deleted env group 'shared'\n"), "{out:?}");

    // A missing resource fails before any prompt.
    let out = ferry(&url, h.path(), &["delete", "nope"]).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no route GET /api/v1/services/nope"), "{}", out.stderr);
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

#[tokio::test(flavor = "multi_thread")]
async fn env_set_shows_and_follows_the_restart_deploy() {
    let (fake, url) = Fake::start().await;
    let mut web = service_view("web", ServiceType::WebService);
    web.service.live_deploy_id = Some("dep-old".into());
    web.latest_deploy = Some(deploy("dep-old", "srv-web", DeployStatus::Live));
    fake.on("GET", "/api/v1/services/web", Reply::ok(to_json(&web)));
    fake.on("GET", "/api/v1/services", Reply::ok(json!([to_json(&web)])));
    fake.on("GET", "/api/v1/datastores", Reply::ok(json!([])));
    fake.on("GET", "/api/v1/services/srv-web/env", Reply::ok(json!([])));
    fake.on("PATCH", "/api/v1/services/srv-web/env", Reply::ok(json!([{ "key": "A", "value": "1" }])));
    // After the change, the service's latest deploy is the restart.
    let mut restart = deploy("dep-new", "srv-web", DeployStatus::Queued);
    restart.trigger = DeployTrigger::EnvChange;
    let mut after = web.clone();
    after.latest_deploy = Some(restart.clone());
    fake.on("GET", "/api/v1/services/srv-web", Reply::ok(to_json(&after)));
    let h = home();

    // The queued restart is shown like `ferry restart` shows it.
    let out = ferry(&url, h.path(), &["env", "set", "web", "A=1"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "A=1\nDeploy dep-new queued (env_change)\nFollow it with: ferry logs --deploy dep-new -f\n");
    assert!(out.stderr.contains("Saved. Restarting 'web' with the new environment."), "{}", out.stderr);
    assert_eq!(fake.find("PATCH", "/api/v1/services/srv-web/env")[0].query, "restart=true");

    // --follow streams it and fails like the deploy does.
    fake.on(
        "GET",
        "/api/v1/deploys/dep-new/logs",
        Reply::sse(&[&log_event("system", None, "==> Restarting with the new environment"), "event: end\n\n"]),
    );
    let mut failed = restart.clone();
    failed.status = DeployStatus::DeployFailed;
    failed.error = Some("env var DB: reference to unknown datastore 'pgx'".into());
    fake.on("GET", "/api/v1/deploys/dep-new", Reply::ok(to_json(&failed)));
    let out = ferry(&url, h.path(), &["env", "set", "web", "DB=${{datastore.pgx.connectionString}}", "-f"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    // Unknown targets are flagged before saving.
    assert!(
        out.stderr.contains("warning: DB: reference to unknown datastore 'pgx'; deploys fail until it resolves"),
        "{}",
        out.stderr
    );
    assert!(out.stdout.contains("==> Restarting with the new environment"), "{}", out.stdout);
    assert!(
        out.stdout.contains("Deploy dep-new failed (deploy_failed): env var DB: reference to unknown datastore 'pgx'"),
        "{}",
        out.stdout
    );

    // A reference that can never resolve is refused before anything is sent.
    let before = fake.requests().len();
    let out = ferry(&url, h.path(), &["env", "set", "web", "TPL=Hello ${{ name }}"]).await;
    assert_eq!(out.code, 2, "{out:?}");
    assert!(out.stderr.contains("TPL: invalid reference"), "{}", out.stderr);
    assert_eq!(fake.requests().len(), before);

    // Never deployed: nothing restarts, and it says so.
    let idle = to_json(&service_view("idle", ServiceType::WebService));
    fake.on("GET", "/api/v1/services/idle", Reply::ok(idle.clone()));
    fake.on("GET", "/api/v1/services/srv-idle", Reply::ok(idle));
    fake.on(
        "GET",
        "/api/v1/services/srv-idle/env",
        Reply::ok(json!([{ "key": "A", "value": "1" }, { "key": "B", "value": "2" }])),
    );
    fake.on("PATCH", "/api/v1/services/srv-idle/env", Reply::ok(json!([{ "key": "A", "value": "1" }])));
    let out = ferry(&url, h.path(), &["env", "unset", "idle", "B"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "A=1\n");
    assert!(out.stderr.contains("'idle' isn't live, so nothing restarts"), "{}", out.stderr);
    assert!(!out.stderr.contains("warning"), "{}", out.stderr);
}

/// `ferry env set` while the first deploy is still running: the server
/// queues a restart behind it, and the CLI says so (it used to claim that
/// nothing restarts because the service wasn't live yet).
#[tokio::test(flavor = "multi_thread")]
async fn env_set_during_the_first_deploy_reports_the_queued_restart() {
    let (fake, url) = Fake::start().await;
    let mut racey = service_view("racey", ServiceType::WebService);
    racey.state = ServiceState::Deploying;
    racey.latest_deploy = Some(deploy("dep-first", "srv-racey", DeployStatus::Building));
    fake.on("GET", "/api/v1/services/racey", Reply::ok(to_json(&racey)));
    fake.on("GET", "/api/v1/services/srv-racey/env", Reply::ok(json!([{ "key": "V", "value": "one" }])));
    fake.on("PATCH", "/api/v1/services/srv-racey/env", Reply::ok(json!([{ "key": "V", "value": "two" }])));
    let mut restart = deploy("dep-r", "srv-racey", DeployStatus::Queued);
    restart.trigger = DeployTrigger::EnvChange;
    let mut after = racey.clone();
    after.latest_deploy = Some(restart);
    fake.on("GET", "/api/v1/services/srv-racey", Reply::ok(to_json(&after)));
    let h = home();

    let out = ferry(&url, h.path(), &["env", "set", "racey", "V=two"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "V=two\nDeploy dep-r queued (env_change)\nFollow it with: ferry logs --deploy dep-r -f\n");
    assert!(
        out.stderr.contains("Saved. 'racey' restarts with the new environment once its current deploy is live."),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("nothing restarts"), "{}", out.stderr);
    assert_eq!(fake.find("PATCH", "/api/v1/services/srv-racey/env")[0].query, "restart=true");

    // A first deploy that is still queued starts with the new environment
    // itself (the server folds the restart into it).
    let mut queued = service_view("q", ServiceType::WebService);
    queued.state = ServiceState::Deploying;
    queued.latest_deploy = Some(deploy("dep-q", "srv-q", DeployStatus::Queued));
    fake.on("GET", "/api/v1/services/q", Reply::ok(to_json(&queued)));
    fake.on("GET", "/api/v1/services/srv-q", Reply::ok(to_json(&queued)));
    fake.on("GET", "/api/v1/services/srv-q/env", Reply::ok(json!([])));
    fake.on("PATCH", "/api/v1/services/srv-q/env", Reply::ok(json!([{ "key": "V", "value": "two" }])));
    let out = ferry(&url, h.path(), &["env", "set", "q", "V=two"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "V=two\nDeploy dep-q queued (manual)\nFollow it with: ferry logs --deploy dep-q -f\n");
    assert!(
        out.stderr.contains("Saved. Deploy dep-q of 'q' is queued: it starts with the new environment."),
        "{}",
        out.stderr
    );

    // --json prints the variables; with --follow it streams the restart.
    let out = ferry(&url, h.path(), &["env", "set", "racey", "V=two", "--json"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap(), json!([{ "key": "V", "value": "two" }]));
}

#[tokio::test(flavor = "multi_thread")]
async fn env_changes_that_change_nothing_say_so_and_restart_nothing() {
    let (fake, url) = Fake::start().await;
    let mut web = service_view("web", ServiceType::WebService);
    web.service.live_deploy_id = Some("dep-1".into());
    web.latest_deploy = Some(deploy("dep-1", "srv-web", DeployStatus::Live));
    web.env_groups = vec!["shared".into()];
    fake.on("GET", "/api/v1/services/web", Reply::ok(to_json(&web)));
    fake.on("GET", "/api/v1/services/srv-web", Reply::ok(to_json(&web)));
    fake.on("GET", "/api/v1/services/srv-web/env", Reply::ok(json!([{ "key": "A", "value": "1" }])));
    let h = home();

    // Replies of the three PATCHes below, in order.
    fake.on("PATCH", "/api/v1/services/srv-web/env", Reply::ok(json!([{ "key": "A", "value": "1" }])));
    fake.on("PATCH", "/api/v1/services/srv-web/env", Reply::ok(json!([{ "key": "A", "value": "1" }])));
    fake.on("PATCH", "/api/v1/services/srv-web/env", Reply::ok(json!([])));

    // Same value: saved without a restart request.
    let out = ferry(&url, h.path(), &["env", "set", "web", "A=1"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "A=1\n");
    assert_eq!(out.stderr, "No changes.\n");

    // Removing a variable that isn't set: a warning, and no restart either.
    let out = ferry(&url, h.path(), &["env", "unset", "web", "NOPE"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stderr.contains("warning: not set on 'web', nothing to remove: NOPE"), "{}", out.stderr);
    assert!(out.stderr.contains("ferry env-group unset"), "linked groups are mentioned: {}", out.stderr);
    assert!(out.stderr.ends_with("No changes.\n"), "{}", out.stderr);
    let patches = fake.find("PATCH", "/api/v1/services/srv-web/env");
    assert_eq!((patches[0].query.as_str(), patches[1].query.as_str()), ("restart=false", "restart=false"));
    assert_eq!(patches[1].json(), json!({ "set": [], "unset": ["NOPE"] }));
    assert!(fake.find("GET", "/api/v1/services/srv-web").is_empty(), "no restart to look for");

    // Some keys missing, others removed: warned about, and it restarts.
    let out = ferry(&url, h.path(), &["env", "unset", "web", "NOPE", "A", "ALSO_NOPE"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stderr.contains("nothing to remove: NOPE, ALSO_NOPE"), "{}", out.stderr);
    assert_eq!(fake.find("PATCH", "/api/v1/services/srv-web/env")[2].query, "restart=true");
    // The server queued nothing (same latest deploy): it says so.
    assert!(out.stderr.contains("Saved, but no restart was queued"), "{}", out.stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn env_listing_quotes_values_and_shows_linked_groups() {
    let (fake, url) = Fake::start().await;
    let own = json!([{ "key": "NL", "value": "line1\nline2" }, { "key": "A", "value": "1" }]);
    fake.on("GET", "/api/v1/services/web/env", Reply::ok(own.clone()));
    fake.on("GET", "/api/v1/services/srv-web/env", Reply::ok(own));
    let mut web = service_view("web", ServiceType::WebService);
    web.env_groups = vec!["shared".into()];
    fake.on("GET", "/api/v1/services/web", Reply::ok(to_json(&web)));
    let group = json!({
        "id": "evg-shared", "name": "shared", "created_at": "2026-01-10T12:00:00Z",
        "updated_at": "2026-01-10T12:00:00Z", "services": ["web"],
        "vars": [{ "key": "G1", "value": "from-group" }, { "key": "A", "value": "overridden" }]
    });
    fake.on("GET", "/api/v1/env-groups/shared", Reply::ok(group));
    let h = home();

    let out = ferry(&url, h.path(), &["env", "web"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "NL=\"line1\\nline2\"\nA=1\n", "one line per variable");
    assert!(out.stderr.contains("Also inherited from linked env group(s) shared"), "{}", out.stderr);
    assert!(out.stderr.contains("ferry env web --effective"), "{}", out.stderr);

    // The merged environment: groups first, the service's own variables win.
    let out = ferry(&url, h.path(), &["env", "ls", "web", "--effective"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "G1=from-group\nA=1\nNL=\"line1\\nline2\"\n");
    let out = ferry(&url, h.path(), &["env", "web", "--effective", "--json"]).await;
    assert_eq!(
        serde_json::from_str::<Value>(&out.stdout).unwrap(),
        json!([
            { "key": "G1", "value": "from-group" },
            { "key": "A", "value": "1" },
            { "key": "NL", "value": "line1\nline2" }
        ])
    );
    // --json keeps the raw values.
    let out = ferry(&url, h.path(), &["env", "web", "--json"]).await;
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap()[0]["value"], "line1\nline2");
}

#[tokio::test(flavor = "multi_thread")]
async fn domain_messages_use_the_normalized_domain() {
    let (fake, url) = Fake::start().await;
    fake.on("POST", "/api/v1/services/echo2/domains", Reply::ok(json!(["spaced.com"])));
    fake.on("DELETE", "/api/v1/services/echo2/domains/newsvc.localhost", Reply::ok(json!(["spaced.com"])));
    let h = home();
    let out = ferry(&url, h.path(), &["domains", "add", "echo2", " Spaced.COM "]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stderr.starts_with("Added spaced.com. Point its DNS"), "{}", out.stderr);
    assert_eq!(fake.find("POST", "/api/v1/services/echo2/domains")[0].json(), json!({ "domain": "spaced.com" }));
    let out = ferry(&url, h.path(), &["domains", "rm", "echo2", "NEWSVC.localhost."]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stderr, "Removed newsvc.localhost.\n");
    assert_eq!(out.stdout, "spaced.com\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_repo_paths_are_sent_as_absolute_paths() {
    let (fake, url) = Fake::start().await;
    let mut created = service_view("relrepo", ServiceType::BackgroundWorker);
    created.state = ServiceState::NotDeployed;
    fake.on("POST", "/api/v1/services", Reply::json(201, to_json(&created)));
    fake.on("PATCH", "/api/v1/services/relrepo", Reply::ok(to_json(&created)));
    let result = json!({ "dry_run": true, "actions": [], "deploys": [], "warnings": [] });
    fake.on("POST", "/api/v1/blueprints/apply", Reply::ok(result));
    let work = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(work.path().join("git/mono/worker")).unwrap();
    let mono = std::fs::canonicalize(work.path().join("git/mono")).unwrap();
    let mono = mono.to_str().unwrap();
    let h = home();
    let run = |args: &'static [&'static str]| ferry_in(Some(work.path()), h.path(), Some(&url), Some(TOKEN), args);

    let out = run(&["create", "relrepo", "--type", "worker", "--repo", "./git/mono", "--no-deploy"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stderr.contains(&format!("note: using the local repository {mono}")), "{}", out.stderr);
    assert_eq!(fake.find("POST", "/api/v1/services")[0].json()["repo_url"], mono);

    let out = run(&["update", "relrepo", "--repo", "git/mono"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let out = run(&["update", "relrepo", "--repo", ""]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let patches = fake.find("PATCH", "/api/v1/services/relrepo");
    assert_eq!(patches[0].json()["repo_url"], mono);
    assert_eq!(patches[1].json()["repo_url"], "", "'' still clears the repo");

    // A relative path that doesn't exist here is refused locally.
    let out = run(&["create", "relcli", "--repo", "./echo"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(out.stderr.contains("--repo './echo': no such directory here"), "{}", out.stderr);
    assert_eq!(fake.find("POST", "/api/v1/services").len(), 1);
    // URLs are sent as given.
    let out = run(&["create", "remote", "--repo", "git@github.com:a/b.git", "--no-deploy"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(fake.find("POST", "/api/v1/services")[1].json()["repo_url"], "git@github.com:a/b.git");

    // Blueprints: relative to the blueprint file's directory.
    std::fs::create_dir_all(work.path().join("git/deploy")).unwrap();
    std::fs::write(
        work.path().join("git/deploy/ferry.yaml"),
        "services:\n  - type: worker\n    name: relrepo\n    repo: ../mono\n    rootDir: worker\n",
    )
    .unwrap();
    let out = run(&["blueprint", "apply", "git/deploy/ferry.yaml", "--dry-run"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stderr.contains("note: service 'relrepo': repo '../mono' → "), "{}", out.stderr);
    let sent = fake.find("POST", "/api/v1/blueprints/apply")[0].json();
    let yaml: serde_yaml::Value = serde_yaml::from_str(sent["yaml"].as_str().unwrap()).unwrap();
    assert_eq!(yaml["services"][0]["repo"].as_str(), Some(mono));
    assert_eq!(yaml["services"][0]["rootDir"].as_str(), Some("worker"));
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_logs_explain_when_nothing_runs() {
    let (fake, url) = Fake::start().await;
    let status = RuntimeStatus {
        service_id: "srv-unknown".into(),
        state: ServiceState::Failed,
        desired_instances: 1,
        instances: vec![],
    };
    fake.on("GET", "/api/v1/services/unknown/status", Reply::ok(to_json(&status)));
    let mut view = service_view("unknown", ServiceType::WebService);
    view.state = ServiceState::Failed;
    let mut failed = deploy("dep-9", "srv-unknown", DeployStatus::BuildFailed);
    failed.error = Some("cannot determine how to start this Python app".into());
    view.latest_deploy = Some(failed);
    fake.on("GET", "/api/v1/services/unknown", Reply::ok(to_json(&view)));
    fake.on("GET", "/api/v1/services/unknown/logs", Reply::sse(&["event: end\ndata:\n\n"]));
    let h = home();
    let out = ferry(&url, h.path(), &["logs", "unknown"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.is_empty());
    assert!(
        out.stderr.contains("its latest deploy dep-9 failed (build_failed: cannot determine how to start")
            && out.stderr.contains("See its log with 'ferry logs --deploy dep-9'"),
        "{}",
        out.stderr
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_hook_is_shown_and_rotated() {
    let (fake, url) = Fake::start().await;
    let web = service_view("web", ServiceType::WebService);
    fake.on("GET", "/api/v1/services/web", Reply::ok(to_json(&web)));
    fake.on("GET", "/api/v1/services/srv-web", Reply::ok(to_json(&web)));
    let mut rotated = web.clone();
    rotated.deploy_hook_path = "/hooks/deploy/srv-web?key=fresh".into();
    fake.on("POST", "/api/v1/services/srv-web/deploy-hook/rotate", Reply::ok(to_json(&rotated)));
    let h = home();
    let old_url = format!("{url}/hooks/deploy/srv-web?key=secret");
    let new_url = format!("{url}/hooks/deploy/srv-web?key=fresh");

    // `ferry show` lists the hook URL (server URL + the hook path).
    let out = ferry(&url, h.path(), &["show", "web"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let line = out.stdout.lines().find(|l| l.starts_with("Deploy hook:")).expect("a deploy hook line");
    assert!(line.ends_with(&format!(" {old_url}")), "{line}");
    // `deploy-hook show` prints just the URL (for scripts).
    let out = ferry(&url, h.path(), &["deploy-hook", "show", "web"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, format!("{old_url}\n").as_str()), "{out:?}");

    // Rotating breaks whatever uses the old URL: confirmation required.
    let out = ferry(&url, h.path(), &["deploy-hook", "rotate", "web"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(out.stderr.contains("refusing to rotate the deploy hook of 'web'"), "{}", out.stderr);
    assert!(fake.find("POST", "/api/v1/services/srv-web/deploy-hook/rotate").is_empty());

    let out = ferry(&url, h.path(), &["deploy-hook", "rotate", "web", "--yes"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, format!("{new_url}\n"));
    assert!(out.stderr.contains("Rotated the deploy hook of 'web': the old URL no longer works"), "{}", out.stderr);
    let post = &fake.find("POST", "/api/v1/services/srv-web/deploy-hook/rotate")[0];
    assert!(post.body.is_empty());

    let out = ferry(&url, h.path(), &["deploy-hook", "rotate", "srv-web", "-y", "--json"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap()["deploy_hook_path"], rotated.deploy_hook_path);

    // An older server without the endpoint.
    let old = service_view("old", ServiceType::WebService);
    fake.on("GET", "/api/v1/services/old", Reply::ok(to_json(&old)));
    fake.on(
        "POST",
        "/api/v1/services/srv-old/deploy-hook/rotate",
        Reply::error(404, "not_found", "no API route for /api/v1/services/srv-old/deploy-hook/rotate"),
    );
    let out = ferry(&url, h.path(), &["deploy-hook", "rotate", "old", "--yes"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(out.stderr.contains("this Ferry server can't rotate deploy hooks: upgrade ferryd"), "{}", out.stderr);
}

fn job(id: &str, status: JobStatus) -> JobRun {
    let mut j = JobRun::new("srv-nightly", JobTrigger::Schedule, None);
    j.id = id.to_string();
    j.status = status;
    j
}

#[tokio::test(flavor = "multi_thread")]
async fn jobs_cancel_stops_a_run_and_explains_finished_ones() {
    let (fake, url) = Fake::start().await;
    fake.on("GET", "/api/v1/jobs/job-1", Reply::ok(to_json(&job("job-1", JobStatus::Running))));
    fake.on("POST", "/api/v1/jobs/job-1/cancel", Reply::ok(to_json(&job("job-1", JobStatus::Canceled))));
    let h = home();

    let out = ferry(&url, h.path(), &["jobs", "cancel", "job-1"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "Job job-1 canceled\n");
    assert!(fake.find("POST", "/api/v1/jobs/job-1/cancel")[0].body.is_empty());
    let out = ferry(&url, h.path(), &["jobs", "cancel", "job-1", "--json"]).await;
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap()["status"], "canceled");

    // Already finished: nothing is sent.
    fake.on("GET", "/api/v1/jobs/job-2", Reply::ok(to_json(&job("job-2", JobStatus::Succeeded))));
    let out = ferry(&url, h.path(), &["jobs", "cancel", "job-2"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert_eq!(out.stderr.trim(), "error: job job-2 already finished (succeeded): nothing to cancel");
    assert!(fake.find("POST", "/api/v1/jobs/job-2/cancel").is_empty());

    // Finished between the lookup and the cancel: the server answers 409.
    fake.on("GET", "/api/v1/jobs/job-3", Reply::ok(to_json(&job("job-3", JobStatus::Running))));
    fake.on("GET", "/api/v1/jobs/job-3", Reply::ok(to_json(&job("job-3", JobStatus::Failed))));
    fake.on("POST", "/api/v1/jobs/job-3/cancel", Reply::error(409, "conflict", "job job-3 already finished (failed)"));
    let out = ferry(&url, h.path(), &["jobs", "cancel", "job-3"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert_eq!(out.stderr.trim(), "error: job job-3 already finished (failed): nothing to cancel");

    // An older server without the endpoint.
    fake.on("GET", "/api/v1/jobs/job-4", Reply::ok(to_json(&job("job-4", JobStatus::Pending))));
    fake.on(
        "POST",
        "/api/v1/jobs/job-4/cancel",
        Reply::error(404, "not_found", "no API route for /api/v1/jobs/job-4/cancel"),
    );
    let out = ferry(&url, h.path(), &["jobs", "cancel", "job-4"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(out.stderr.contains("this Ferry server can't cancel jobs: upgrade ferryd"), "{}", out.stderr);

    // Unknown job: the lookup fails, nothing is canceled.
    let out = ferry(&url, h.path(), &["jobs", "cancel", "job-nope"]).await;
    assert_eq!(out.code, 1, "{out:?}");
    assert!(fake.find("POST", "/api/v1/jobs/job-nope/cancel").is_empty());

    // Listing still works, with or without `ls`.
    fake.on("GET", "/api/v1/services/nightly/jobs", Reply::ok(json!([to_json(&job("job-1", JobStatus::Canceled))])));
    let out = ferry(&url, h.path(), &["jobs", "nightly"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.lines().nth(1).is_some_and(|l| l.starts_with("job-1   canceled   schedule")), "{}", out.stdout);
    let out = ferry(&url, h.path(), &["jobs", "ls", "nightly", "-n", "5"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    let lists = fake.find("GET", "/api/v1/services/nightly/jobs");
    assert_eq!((lists[0].query.as_str(), lists[1].query.as_str()), ("limit=20", "limit=5"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cron_start_command_applies_from_the_next_run() {
    let (fake, url) = Fake::start().await;
    let mut cron = service_view("nightly", ServiceType::CronJob);
    cron.service.schedule = Some("0 3 * * *".into());
    cron.service.start_command = Some("echo v2".into());
    fake.on("PATCH", "/api/v1/services/nightly", Reply::ok(to_json(&cron)));
    let h = home();
    let out = ferry(&url, h.path(), &["update", "nightly", "--start-cmd", "echo v2"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(
        out.stdout,
        "Updated 'nightly'\nThe new start command applies from the next run (scheduled, or now with: ferry run nightly)\n"
    );
    assert_eq!(fake.find("PATCH", "/api/v1/services/nightly")[0].json()["start_command"], "echo v2");
}

#[tokio::test(flavor = "multi_thread")]
async fn update_help_says_how_to_clear_the_port() {
    let h = home();
    let out = ferry_in(None, h.path(), None, None, &["update", "--help"]).await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(out.stdout.contains("--port 0 clears the port setting"), "{}", out.stdout);
    assert!(out.stdout.contains("--port 0 to clear the port setting"), "{}", out.stdout);
}
