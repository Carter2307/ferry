//! Docker-gated end-to-end tests: the real stack (store, Docker, builder,
//! engine, proxy) booted in-process. They only run with `FERRY_E2E=1`
//! (otherwise each test prints "skipped" and returns).
//!
//! Every test uses its own `name_prefix` (`ferrytest-engine-<random>`), data
//! directory and proxy port, and a drop guard removes every container,
//! volume, image and network with that prefix even when an assertion fails.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ferry_build::Builder;
use ferry_core::{
    CancellationToken, Config, Datastore, DatastoreKind, DatastoreStatus, Deploy, DeployRequest, DeploySource,
    DeployStatus, DeployTrigger, Engine, JobStatus, JobTrigger, LogLine, LogOptions, Service, ServiceState,
    ServiceType, Store, ids, validate,
};
use ferry_docker::Docker;
use ferry_engine::FerryEngine;
use ferry_proxy::{ProxyConfig, RouteTable};
use futures::StreamExt;

fn e2e_enabled() -> bool {
    std::env::var("FERRY_E2E").is_ok_and(|v| v == "1")
}

macro_rules! require_e2e {
    () => {
        if !e2e_enabled() {
            eprintln!("skipped: set FERRY_E2E=1 to run the engine's Docker tests");
            return;
        }
    };
}

const BUILD_TIMEOUT: Duration = Duration::from_secs(300);

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Run the docker CLI; (success, stdout).
fn docker_cli(args: &[&str]) -> (bool, String) {
    match Command::new("docker").args(args).output() {
        Ok(out) => (out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_string()),
        Err(_) => (false, String::new()),
    }
}

fn run(cmd: &str, args: &[&str], dir: &Path) -> String {
    let out = Command::new(cmd).args(args).current_dir(dir).output().unwrap_or_else(|e| panic!("{cmd}: {e}"));
    assert!(out.status.success(), "{cmd} {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Removes every Docker object of a test prefix, even on panic.
struct Cleanup {
    prefix: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let (_, ids) = docker_cli(&["ps", "-aq", "--filter", &format!("label=ferry.instance={}", self.prefix)]);
        let ids: Vec<&str> = ids.split_whitespace().collect();
        if !ids.is_empty() {
            let mut args = vec!["rm", "-f", "-v"];
            args.extend(ids);
            docker_cli(&args);
        }
        let (_, volumes) = docker_cli(&["volume", "ls", "-q"]);
        for v in volumes.lines().filter(|v| v.starts_with(&format!("{}-", self.prefix))) {
            docker_cli(&["volume", "rm", "-f", v]);
        }
        let (_, images) = docker_cli(&["images", "--format", "{{.Repository}}:{{.Tag}}"]);
        for i in images.lines().filter(|i| i.starts_with(&format!("{}/", self.prefix))) {
            docker_cli(&["rmi", "-f", i]);
        }
        docker_cli(&["network", "rm", &self.prefix]);
    }
}

struct Harness {
    engine: Arc<FerryEngine>,
    store: Store,
    config: Arc<Config>,
    routes: RouteTable,
    http: reqwest::Client,
    proxy: SocketAddr,
    shutdown: CancellationToken,
    prefix: String,
    dir: tempfile::TempDir,
    _cleanup: Cleanup,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn harness(tweak: impl FnOnce(&mut Config)) -> Harness {
    if std::env::var("RUST_LOG").is_ok() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
    let prefix = format!("ferrytest-engine-{}", ids::random_secret(8));
    let cleanup = Cleanup { prefix: prefix.clone() };
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().canonicalize().unwrap();
    let proxy: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut config = Config {
        data_dir,
        api_addr: format!("127.0.0.1:{}", free_port()).parse().unwrap(),
        proxy_addr: proxy,
        dashboard_host: None,
        name_prefix: prefix.clone(),
        health_check_timeout_secs: 90,
        keep_images: 3,
        build_concurrency: 2,
        ..Config::default()
    };
    tweak(&mut config);
    let config = Arc::new(config);
    let store = Store::open(&config.db_path()).await.unwrap();
    let docker = Docker::connect().await.expect("Docker must be running for FERRY_E2E tests");
    let builder = Builder::new(config.builds_dir(), config.repos_dir(), config.docker_bin.clone());
    let routes = RouteTable::new();
    let engine = FerryEngine::new(config.clone(), store.clone(), docker, builder, routes.clone());
    let shutdown = CancellationToken::new();
    engine.start(shutdown.clone()).await.expect("engine start");
    {
        let (routes, shutdown) = (routes.clone(), shutdown.clone());
        tokio::spawn(async move {
            if let Err(e) = ferry_proxy::serve(ProxyConfig::http(proxy), routes, shutdown).await {
                eprintln!("proxy failed: {e}");
            }
        });
    }
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(proxy).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    Harness { engine, store, config, routes, http, proxy, shutdown, prefix, dir, _cleanup: cleanup }
}

impl Harness {
    fn host(&self, name: &str) -> String {
        self.config.default_host(name)
    }

    /// GET through the proxy with the service's Host header.
    async fn get(&self, name: &str, path: &str) -> Result<(u16, String), String> {
        let url = format!("http://{}{path}", self.proxy);
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::HOST, self.host(name))
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp.text().await.map_err(|e| format!("reading body: {e}"))?;
        Ok((status, body))
    }

    /// Poll until `GET path` answers 200 (returns the body).
    async fn wait_ok(&self, name: &str, path: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        while Instant::now() < deadline {
            match self.get(name, path).await {
                Ok((200, body)) => return body,
                Ok((s, body)) => last = format!("HTTP {s}: {}", body.chars().take(200).collect::<String>()),
                Err(e) => last = e,
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("GET {name}{path} never answered 200 (last: {last})");
    }

    async fn create_service(&self, name: &str, kind: ServiceType, f: impl FnOnce(&mut Service)) -> Service {
        let mut s = Service::new(name, kind);
        f(&mut s);
        validate::normalize_service(&mut s);
        validate::service(&s).unwrap();
        self.store.create_service(&s).await.unwrap();
        s
    }

    async fn deploy_log(&self, id: &str) -> String {
        let lines: Vec<LogLine> = self.engine.deploy_logs(id, false).await.unwrap().collect().await;
        lines.iter().map(|l| l.line.as_str()).collect::<Vec<_>>().join("\n")
    }

    async fn wait_terminal(&self, id: &str) -> Deploy {
        let deadline = Instant::now() + BUILD_TIMEOUT;
        loop {
            let d = self.store.require_deploy(id).await.unwrap();
            if d.status.is_terminal() {
                return d;
            }
            assert!(Instant::now() < deadline, "deploy {id} still {} after {BUILD_TIMEOUT:?}", d.status);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// Wait for a deploy and require the given final status (printing its
    /// log otherwise).
    async fn expect(&self, d: &Deploy, status: DeployStatus) -> Deploy {
        let done = self.wait_terminal(&d.id).await;
        if done.status != status {
            panic!(
                "deploy {} ended {} (expected {status}): {:?}\n--- log ---\n{}",
                done.id,
                done.status,
                done.error,
                self.deploy_log(&done.id).await
            );
        }
        done
    }

    async fn deploy_live(&self, svc: &Service, req: DeployRequest) -> Deploy {
        let d = self.engine.deploy(&svc.id, req).await.unwrap();
        self.expect(&d, DeployStatus::Live).await
    }

    /// Containers with our prefix and the given extra label filter.
    fn containers(&self, label: &str) -> Vec<String> {
        let (_, out) = docker_cli(&[
            "ps",
            "-aq",
            "--filter",
            &format!("label=ferry.instance={}", self.prefix),
            "--filter",
            &format!("label={label}"),
        ]);
        out.split_whitespace().map(str::to_string).collect()
    }

    fn own_images(&self, service_name: &str) -> Vec<String> {
        let repo = format!("{}/{service_name}", self.prefix);
        let (_, out) = docker_cli(&["images", "--format", "{{.Repository}}:{{.Tag}}"]);
        out.lines().filter(|l| l.starts_with(&format!("{repo}:"))).map(str::to_string).collect()
    }

    fn upstreams(&self, name: &str) -> usize {
        let host = self.host(name);
        self.routes.snapshot().into_iter().find(|r| r.host == host).map_or(0, |r| r.upstreams.len())
    }

    async fn wait_until<F, Fut>(&self, what: &str, timeout: Duration, mut f: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if f().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        panic!("timed out waiting for {what}");
    }

    async fn wait_datastore(&self, id: &str, timeout: Duration) -> Datastore {
        let deadline = Instant::now() + timeout;
        loop {
            let ds = self.store.require_datastore(id).await.unwrap();
            match ds.status {
                DatastoreStatus::Available => return ds,
                DatastoreStatus::Failed => panic!("datastore {} failed: {:?}", ds.name, ds.error),
                DatastoreStatus::Creating => {}
            }
            assert!(Instant::now() < deadline, "datastore {} still creating", ds.name);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    async fn wait_job(&self, id: &str) -> ferry_core::JobRun {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let j = self.store.require_job_run(id).await.unwrap();
            if j.status.is_terminal() {
                return j;
            }
            assert!(Instant::now() < deadline, "job {id} still {}", j.status);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// A git repository with a copy of an example app.
    fn git_repo(&self, example: &str) -> PathBuf {
        let repo = self.dir.path().join(format!("repo-{example}"));
        std::fs::create_dir_all(&repo).unwrap();
        let src = examples_dir().join(example);
        run("cp", &["-R", &format!("{}/.", src.display()), &repo.display().to_string()], &repo);
        run("git", &["init", "-q", "-b", "main"], &repo);
        run("git", &["add", "-A"], &repo);
        git_commit(&repo, "initial");
        repo
    }
}

fn git_commit(repo: &Path, msg: &str) -> String {
    run("git", &["-c", "user.name=Ferry Test", "-c", "user.email=test@ferry.invalid", "commit", "-q", "-m", msg], repo);
    run("git", &["rev-parse", "HEAD"], repo)
}

fn env_value(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("value").and_then(|v| v.as_str()).map(str::to_string)
}

// ---------------------------------------------------------------------------
// (1) image web service, (7) suspend/resume, (8) scale, (13) reconciler
// replaces a killed container, (14) delete.

#[tokio::test(flavor = "multi_thread")]
async fn image_web_service_lifecycle() {
    require_e2e!();
    let h = harness(|_| {}).await;
    let svc = h.create_service("web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;

    // (1) live, 200 through the proxy.
    let d1 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    assert_eq!(d1.port, Some(80), "nginx EXPOSEs 80");
    assert_eq!(d1.image.as_deref(), Some("nginx:alpine"));
    let body = h.wait_ok("web", "/", Duration::from_secs(30)).await;
    assert!(body.contains("nginx"), "{body}");
    let log = h.deploy_log(&d1.id).await;
    for expected in
        ["==> Using port 80", "==> Starting 1 instance(s)", "==> Health check passed", "==> Your service is live"]
    {
        assert!(log.contains(expected), "missing {expected:?} in:\n{log}");
    }
    assert_eq!(h.store.require_service(&svc.id).await.unwrap().live_deploy_id.as_deref(), Some(d1.id.as_str()));
    let status = h.engine.service_status(&svc.id).await.unwrap();
    assert_eq!(status.state, ServiceState::Live);
    assert_eq!(status.instances.len(), 1);
    assert_eq!(status.instances[0].state, "running");
    assert!(status.instances[0].started_at.is_some());

    // (8) scale 1 → 3 → 1.
    h.engine.scale(&svc.id, 3).await.unwrap();
    h.wait_until("3 routed instances", Duration::from_secs(60), || async { h.upstreams("web") == 3 }).await;
    let status = h.engine.service_status(&svc.id).await.unwrap();
    assert_eq!(status.desired_instances, 3);
    assert_eq!(status.instances.iter().filter(|i| i.state == "running").count(), 3);
    assert!(status.instances.iter().all(|i| i.deploy_id.as_deref() == Some(d1.id.as_str())));
    h.engine.scale(&svc.id, 1).await.unwrap();
    assert_eq!(h.upstreams("web"), 1);
    assert_eq!(h.engine.service_status(&svc.id).await.unwrap().instances.len(), 1);
    h.wait_ok("web", "/", Duration::from_secs(10)).await;

    // (13) a container killed behind Ferry's back is replaced.
    let before = h.engine.service_status(&svc.id).await.unwrap().instances[0].container_id.clone();
    assert!(docker_cli(&["kill", &before]).0);
    h.wait_until("a replacement instance", Duration::from_secs(60), || async {
        let st = h.engine.service_status(&svc.id).await.unwrap();
        st.instances.len() == 1 && st.instances[0].container_id != before && st.instances[0].state == "running"
    })
    .await;
    h.wait_ok("web", "/", Duration::from_secs(30)).await;

    // (7) suspend → 503, resume → 200.
    h.engine.suspend(&svc.id).await.unwrap();
    let (status, _) = h.get("web", "/").await.unwrap();
    assert_eq!(status, 503);
    assert!(h.containers(&format!("ferry.service={}", svc.id)).is_empty());
    let st = h.engine.service_status(&svc.id).await.unwrap();
    assert_eq!(st.state, ServiceState::Suspended);
    assert!(h.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.is_err());
    h.engine.resume(&svc.id).await.unwrap();
    h.wait_ok("web", "/", Duration::from_secs(60)).await;

    // (14) delete: containers, routes, rows.
    h.engine.delete_service(&svc.id).await.unwrap();
    assert!(h.containers(&format!("ferry.service={}", svc.id)).is_empty());
    let (status, _) = h.get("web", "/").await.unwrap();
    assert_eq!(status, 404);
    assert!(h.store.find_service(&svc.id).await.unwrap().is_none());
    assert!(h.store.get_deploy(&d1.id).await.unwrap().is_none());
    // The pulled public image is never deleted.
    assert!(docker_cli(&["image", "inspect", "nginx:alpine"]).0);
}

// ---------------------------------------------------------------------------
// (2) git deploy with 2 instances, (3) zero-downtime redeploy, (4) failing
// deploy keeps the previous one serving, (5) rollback, (6) env change +
// restart and a redis reference, (14) delete removes images.

#[tokio::test(flavor = "multi_thread")]
async fn git_service_redeploys_rollback_and_env() {
    require_e2e!();
    let h = harness(|_| {}).await;

    // A redis datastore the service references.
    let cache = Datastore::new("cache", DatastoreKind::Redis);
    h.store.create_datastore(&cache).await.unwrap();
    h.engine.provision_datastore(&cache.id).await.unwrap();
    let cache = h.wait_datastore(&cache.id, Duration::from_secs(120)).await;

    let repo = h.git_repo("docker-echo");
    let svc = h
        .create_service("echo", ServiceType::WebService, |s| {
            s.repo_url = Some(repo.display().to_string());
            s.instances = 2;
            s.health_check_path = Some("/healthz".into());
        })
        .await;
    h.store.set_env(&svc.id, "REDIS_URL", "${{datastore.cache.connectionString}}").await.unwrap();
    h.store.set_env(&svc.id, "GREETING", "one").await.unwrap();

    // (2) build from git, two instances behind the proxy.
    let d1 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    assert_eq!(d1.port, Some(8000), "EXPOSE 8000");
    assert_eq!(d1.commit_sha.as_deref().map(str::len), Some(40));
    assert_eq!(d1.commit_message.as_deref(), Some("initial"));
    let log = h.deploy_log(&d1.id).await;
    assert!(log.contains("==> Cloning from") && log.contains("==> Build successful"), "{log}");
    assert!(log.contains("docker-echo listening"), "new instances' output is in the deploy log:\n{log}");
    let mut hostnames = std::collections::HashSet::new();
    for _ in 0..40 {
        let body = h.wait_ok("echo", "/", Duration::from_secs(30)).await;
        hostnames.insert(body);
        if hostnames.len() >= 2 {
            break;
        }
    }
    assert_eq!(hostnames.len(), 2, "both instances receive traffic: {hostnames:?}");

    // (6) the datastore reference resolves inside the container, and the
    // private network reaches the datastore by name.
    let url = env_value(&h.wait_ok("echo", "/env?key=REDIS_URL", Duration::from_secs(10)).await);
    assert_eq!(url, Some(cache.internal_url()));
    let echo_container = h.containers(&format!("ferry.deploy={}", d1.id))[0].clone();
    let (ok, _) = docker_cli(&[
        "exec",
        &echo_container,
        "python",
        "-c",
        "import socket; socket.create_connection(('cache', 6379), 5).close()",
    ]);
    assert!(ok, "the service reaches the datastore on the private network");

    // (3) zero-downtime git redeploy: a request loop sees no errors.
    std::fs::write(repo.join("VERSION"), "2\n").unwrap();
    run("git", &["add", "-A"], &repo);
    let sha2 = git_commit(&repo, "second");
    let stop = Arc::new(AtomicBool::new(false));
    let ok_count = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let looper = {
        let (stop, ok_count, failures) = (stop.clone(), ok_count.clone(), failures.clone());
        let (http, proxy, host) = (h.http.clone(), h.proxy, h.host("echo"));
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let res = http.get(format!("http://{proxy}/")).header(reqwest::header::HOST, &host).send().await;
                match res {
                    Ok(r) if r.status().as_u16() == 200 => {
                        ok_count.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(r) => failures.lock().unwrap().push(format!("HTTP {}", r.status())),
                    Err(e) => failures.lock().unwrap().push(e.to_string()),
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    };
    let d2 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Webhook)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    stop.store(true, Ordering::Relaxed);
    looper.await.unwrap();
    let failures = failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "requests failed during the redeploy: {failures:?}");
    assert!(ok_count.load(Ordering::Relaxed) > 20);
    assert_eq!(d2.commit_sha.as_deref(), Some(sha2.as_str()));
    assert_eq!(h.store.require_deploy(&d1.id).await.unwrap().status, DeployStatus::Deactivated);
    assert!(h.containers(&format!("ferry.deploy={}", d1.id)).is_empty(), "old instances are removed");
    assert_eq!(h.upstreams("echo"), 2);

    // (6) env change + restart.
    h.store.set_env(&svc.id, "GREETING", "two").await.unwrap();
    let d3 = h.engine.restart(&svc.id, DeployTrigger::EnvChange).await.unwrap();
    assert!(matches!(d3.source, DeploySource::Reuse { .. }));
    let d3 = h.expect(&d3, DeployStatus::Live).await;
    assert_eq!(d3.commit_sha, d2.commit_sha, "a restart keeps the commit of the image it reuses");
    let greeting = env_value(&h.wait_ok("echo", "/env?key=GREETING", Duration::from_secs(10)).await);
    assert_eq!(greeting.as_deref(), Some("two"));

    // (4) a deploy whose instances crash fails; the previous one keeps serving.
    let mut broken = h.store.require_service(&svc.id).await.unwrap();
    broken.start_command = Some("echo boom-before-exit; exit 3".into());
    h.store.update_service(&broken).await.unwrap();
    let d4 = h.engine.restart(&svc.id, DeployTrigger::Restart).await.unwrap();
    let d4 = h.expect(&d4, DeployStatus::DeployFailed).await;
    let error = d4.error.clone().unwrap_or_default();
    assert!(error.contains("exited") || error.contains("crashed"), "{error}");
    let log = h.deploy_log(&d4.id).await;
    assert!(log.contains("boom-before-exit"), "the crashed instance's output is in the log:\n{log}");
    assert!(log.contains("==> Deploy failed:"), "{log}");
    assert!(h.containers(&format!("ferry.deploy={}", d4.id)).is_empty(), "failed instances are removed");
    assert_eq!(h.store.require_service(&svc.id).await.unwrap().live_deploy_id.as_deref(), Some(d3.id.as_str()));
    let greeting = env_value(&h.wait_ok("echo", "/env?key=GREETING", Duration::from_secs(5)).await);
    assert_eq!(greeting.as_deref(), Some("two"), "still served by the previous deploy");
    broken.start_command = None;
    h.store.update_service(&broken).await.unwrap();

    // (5) rollback to the first deploy's image (with the current env).
    let d5 = h.engine.rollback(&svc.id, &d1.id).await.unwrap();
    assert_eq!(d5.trigger, DeployTrigger::Rollback);
    let d5 = h.expect(&d5, DeployStatus::Live).await;
    assert_eq!(d5.image, d1.image);
    let commit = env_value(&h.wait_ok("echo", "/env?key=FERRY_GIT_COMMIT", Duration::from_secs(10)).await);
    assert_eq!(commit, d1.commit_sha);
    assert_eq!(h.store.require_deploy(&d3.id).await.unwrap().status, DeployStatus::Deactivated);

    // Status and runtime logs.
    let st = h.engine.service_status(&svc.id).await.unwrap();
    assert_eq!(st.state, ServiceState::Live);
    assert_eq!(st.instances.len(), 2);
    assert!(st.instances.iter().all(|i| i.deploy_id.as_deref() == Some(d5.id.as_str())));
    assert!(st.instances.iter().any(|i| i.memory_bytes.is_some()));
    let logs: Vec<LogLine> =
        h.engine.service_logs(&svc.id, LogOptions { follow: false, tail: Some(20) }).await.unwrap().collect().await;
    assert!(logs.iter().any(|l| l.line.contains("docker-echo listening")), "{logs:?}");
    assert!(logs.iter().all(|l| l.instance.as_ref().is_some_and(|i| i.len() == 6)));

    // (14) delete removes containers, routes and the images built for it.
    assert!(!h.own_images("echo").is_empty());
    h.engine.delete_service(&svc.id).await.unwrap();
    assert!(h.containers(&format!("ferry.service={}", svc.id)).is_empty());
    assert!(h.own_images("echo").is_empty(), "{:?}", h.own_images("echo"));
    assert_eq!(h.get("echo", "/").await.unwrap().0, 404);
    assert!(!h.config.repos_dir().join(&svc.id).exists());

    h.engine.delete_datastore(&cache.id).await.unwrap();
    assert!(h.containers(&format!("ferry.datastore={}", cache.id)).is_empty());
    assert!(!docker_cli(&["volume", "inspect", &format!("{}-ds-cache-data", h.prefix)]).0);
    assert!(h.store.find_datastore(&cache.id).await.unwrap().is_none());
}

// ---------------------------------------------------------------------------
// (9) worker with auto-detected python, (10) cron job + one-off jobs.

#[tokio::test(flavor = "multi_thread")]
async fn worker_and_jobs() {
    require_e2e!();
    let h = harness(|_| {}).await;

    // (9) worker from examples/worker (python, Procfile).
    let repo = h.git_repo("worker");
    let worker =
        h.create_service("bg", ServiceType::BackgroundWorker, |s| s.repo_url = Some(repo.display().to_string())).await;
    let d = h.deploy_live(&worker, DeployRequest::new(DeployTrigger::Create)).await;
    assert_eq!(d.port, None, "workers have no port");
    let log = h.deploy_log(&d.id).await;
    assert!(log.contains("Python"), "{log}");
    assert!(log.contains("heartbeat"), "the worker's output during the health check:\n{log}");
    h.wait_until("heartbeat in runtime logs", Duration::from_secs(30), || async {
        let lines: Vec<LogLine> =
            h.engine.service_logs(&worker.id, LogOptions::default()).await.unwrap().collect().await;
        lines.iter().any(|l| l.line.contains("worker bg heartbeat"))
    })
    .await;
    // Following: new lines arrive live.
    let mut follow = h.engine.service_logs(&worker.id, LogOptions { follow: true, tail: Some(0) }).await.unwrap();
    let next = tokio::time::timeout(Duration::from_secs(15), follow.next()).await.unwrap().unwrap();
    assert!(next.line.contains("heartbeat"), "{next:?}");
    drop(follow);
    let st = h.engine.service_status(&worker.id).await.unwrap();
    assert_eq!(st.instances.len(), 1);
    assert!(st.instances[0].host_port.is_none());

    // One-off job on the worker's image: output and exit code are recorded.
    let job = h
        .engine
        .run_job(&worker.id, Some("echo one-off $FERRY_SERVICE_NAME; exit 4".into()), JobTrigger::Manual)
        .await
        .unwrap();
    let job = h.wait_job(&job.id).await;
    assert_eq!(job.status, JobStatus::Failed);
    assert_eq!(job.exit_code, Some(4));
    let lines: Vec<LogLine> = h.engine.job_logs(&job.id, true).await.unwrap().collect().await;
    assert!(lines.iter().any(|l| l.line == "one-off bg"), "{lines:?}");

    // (10) cron job: deploy = pull only; a run executes the start command.
    let cron = h
        .create_service("tick", ServiceType::CronJob, |s| {
            s.image = Some("busybox:stable".into());
            s.schedule = Some("* * * * *".into());
            s.start_command = Some("echo hello from cron".into());
        })
        .await;
    let d = h.deploy_live(&cron, DeployRequest::new(DeployTrigger::Create)).await;
    assert!(h.containers(&format!("ferry.deploy={}", d.id)).is_empty(), "cron deploys start nothing");
    let job = h.engine.run_job(&cron.id, None, JobTrigger::Manual).await.unwrap();
    // Following the job's log ends when it finishes.
    let lines: Vec<LogLine> = tokio::time::timeout(Duration::from_secs(60), async {
        h.engine.job_logs(&job.id, true).await.unwrap().collect::<Vec<_>>().await
    })
    .await
    .unwrap();
    assert!(lines.iter().any(|l| l.line == "hello from cron"), "{lines:?}");
    let job = h.wait_job(&job.id).await;
    assert_eq!(job.status, JobStatus::Succeeded, "{job:?}");
    assert_eq!(job.exit_code, Some(0));
    assert!(h.containers(&format!("ferry.job={}", job.id)).is_empty(), "job containers are removed");

    // The scheduler fires `* * * * *` within about a minute.
    h.wait_until("a scheduled run", Duration::from_secs(100), || async {
        h.store.list_job_runs(&cron.id, 20).await.unwrap().iter().any(|j| j.trigger == JobTrigger::Schedule)
    })
    .await;
}

// ---------------------------------------------------------------------------
// (11) postgres datastore; the reconciler recreates a missing container.

#[tokio::test(flavor = "multi_thread")]
async fn postgres_datastore() {
    require_e2e!();
    let h = harness(|_| {}).await;
    let ds = Datastore::new("pg", DatastoreKind::Postgres);
    h.store.create_datastore(&ds).await.unwrap();
    h.engine.provision_datastore(&ds.id).await.unwrap();
    let ds = h.wait_datastore(&ds.id, Duration::from_secs(180)).await;
    let port = ds.host_port.expect("host port allocated");
    tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("external port accepts TCP");

    let container = format!("{}-ds-pg", h.prefix);
    let psql = |sql: &str| docker_cli(&["exec", &container, "psql", "-U", &ds.username, "-d", "pg", "-tAc", sql]);
    assert!(psql("CREATE TABLE kept (x int); INSERT INTO kept VALUES (42);").0);

    // Removed behind Ferry's back: recreated, data kept by the volume.
    assert!(docker_cli(&["rm", "-f", &container]).0);
    h.wait_until("the datastore container to come back", Duration::from_secs(120), || async {
        let running = docker_cli(&["inspect", "-f", "{{.State.Running}}", &container]).1 == "true";
        running
            && h.store.require_datastore(&ds.id).await.unwrap().status == DatastoreStatus::Available
            && psql("SELECT x FROM kept").1 == "42"
    })
    .await;
    assert_eq!(h.store.require_datastore(&ds.id).await.unwrap().host_port, Some(port), "same external port");

    h.engine.delete_datastore(&ds.id).await.unwrap();
    assert!(!docker_cli(&["inspect", &container]).0);
    assert!(!docker_cli(&["volume", "inspect", &format!("{}-ds-pg-data", h.prefix)]).0);
}

// ---------------------------------------------------------------------------
// (12) archive deploy of a static site; disk + recreate deploys.

#[tokio::test(flavor = "multi_thread")]
async fn archive_static_site_and_disk() {
    require_e2e!();
    let h = harness(|_| {}).await;
    let uploads = h.config.uploads_dir();
    std::fs::create_dir_all(&uploads).unwrap();
    let archive = uploads.join("upl-static.tar.gz");
    run(
        "tar",
        &["-czf", &archive.display().to_string(), "-C", &examples_dir().join("static-site").display().to_string(), "."],
        &uploads,
    );
    let site = h.create_service("site", ServiceType::StaticSite, |_| {}).await;
    let req = DeployRequest {
        trigger: DeployTrigger::Upload,
        source: Some(DeploySource::Archive { path: archive.display().to_string() }),
        commit: None,
        clear_cache: false,
    };
    let d = h.deploy_live(&site, req).await;
    assert_eq!(d.port, Some(80), "static sites are served by nginx on 80");
    let body = h.wait_ok("site", "/", Duration::from_secs(30)).await;
    assert!(body.contains("Hello from a Ferry static site"), "{body}");
    h.wait_ok("site", "/about.html", Duration::from_secs(5)).await;
    // A plain deploy reuses the latest upload.
    let d2 = h.deploy_live(&site, DeployRequest::new(DeployTrigger::Manual)).await;
    assert!(matches!(d2.source, DeploySource::Archive { .. }));
    h.wait_ok("site", "/", Duration::from_secs(30)).await;

    // Disk: data survives recreate deploys.
    let disky = h
        .create_service("disky", ServiceType::WebService, |s| {
            s.image = Some("nginx:alpine".into());
            s.disk_mount_path = Some("/usr/share/nginx/html/data".into());
        })
        .await;
    let d1 = h.deploy_live(&disky, DeployRequest::new(DeployTrigger::Create)).await;
    let c1 = h.containers(&format!("ferry.deploy={}", d1.id))[0].clone();
    assert!(docker_cli(&["exec", &c1, "sh", "-c", "echo persisted > /usr/share/nginx/html/data/f.txt"]).0);
    let d2 = h.engine.restart(&disky.id, DeployTrigger::Restart).await.unwrap();
    let d2 = h.expect(&d2, DeployStatus::Live).await;
    assert!(h.deploy_log(&d2.id).await.contains("redeployed in place"));
    assert!(h.containers(&format!("ferry.deploy={}", d1.id)).is_empty());
    let body = h.wait_ok("disky", "/data/f.txt", Duration::from_secs(30)).await;
    assert_eq!(body.trim(), "persisted");
    let volume = format!("{}-svc-{}-disk", h.prefix, disky.id);
    assert!(docker_cli(&["volume", "inspect", &volume]).0);
    h.engine.delete_service(&disky.id).await.unwrap();
    assert!(!docker_cli(&["volume", "inspect", &volume]).0, "the disk is removed with the service");
}

// ---------------------------------------------------------------------------
// (15) cancel: a queued deploy, a building deploy, a deploying deploy.

#[tokio::test(flavor = "multi_thread")]
async fn cancel_deploys() {
    require_e2e!();
    let h = harness(|c| {
        c.build_concurrency = 1;
        c.health_check_timeout_secs = 120;
    })
    .await;

    // A slow build.
    let repo = h.git_repo("docker-echo");
    let dockerfile = std::fs::read_to_string(repo.join("Dockerfile")).unwrap();
    std::fs::write(repo.join("Dockerfile"), dockerfile.replace("WORKDIR /app", "WORKDIR /app\nRUN sleep 120")).unwrap();
    run("git", &["add", "-A"], &repo);
    git_commit(&repo, "slow");
    let slow =
        h.create_service("slow", ServiceType::WebService, |s| s.repo_url = Some(repo.display().to_string())).await;
    let other =
        h.create_service("other", ServiceType::WebService, |s| s.repo_url = Some(repo.display().to_string())).await;
    let building = h.engine.deploy(&slow.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    h.wait_until("the build to start", Duration::from_secs(60), || async {
        h.deploy_log(&building.id).await.contains("sleep 120")
    })
    .await;

    // Only one build slot: the other service's deploy waits, queued.
    let queued = h.engine.deploy(&other.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.store.require_deploy(&queued.id).await.unwrap().status, DeployStatus::Queued);
    let c = h.engine.cancel_deploy(&queued.id).await.unwrap();
    assert_eq!(c.status, DeployStatus::Canceled);

    // Cancel the running build: docker build is killed.
    let started = Instant::now();
    let c = h.engine.cancel_deploy(&building.id).await.unwrap();
    assert_eq!(c.status, DeployStatus::Canceled, "{c:?}");
    assert!(started.elapsed() < Duration::from_secs(60));
    assert!(c.image.is_none());
    assert!(h.own_images("slow").is_empty());
    assert!(h.engine.cancel_deploy(&building.id).await.is_err(), "terminal deploys can't be canceled");

    // Cancel while deploying (health check that never passes).
    let never = h
        .create_service("never", ServiceType::WebService, |s| {
            s.image = Some("nginx:alpine".into());
            s.health_check_path = Some("/does-not-exist".into());
        })
        .await;
    let d = h.engine.deploy(&never.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    h.wait_until("the deploying phase", Duration::from_secs(60), || async {
        h.store.require_deploy(&d.id).await.unwrap().status == DeployStatus::Deploying
            && !h.containers(&format!("ferry.deploy={}", d.id)).is_empty()
    })
    .await;
    let c = h.engine.cancel_deploy(&d.id).await.unwrap();
    assert_eq!(c.status, DeployStatus::Canceled);
    assert!(h.containers(&format!("ferry.deploy={}", d.id)).is_empty(), "new instances are removed");
    let log = h.deploy_log(&d.id).await;
    assert!(log.contains("==> Deploy canceled"), "{log}");

    // The queue keeps working afterwards.
    let ok = h.create_service("fine", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    h.deploy_live(&ok, DeployRequest::new(DeployTrigger::Manual)).await;
    h.wait_ok("fine", "/", Duration::from_secs(30)).await;
}
