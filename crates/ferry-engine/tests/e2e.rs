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
    harness_named("ferrytest-engine", tweak).await
}

/// Regression tests of fixed findings use their own prefix family.
async fn fix_harness(tweak: impl FnOnce(&mut Config)) -> Harness {
    harness_named("ferryfix-engine", tweak).await
}

async fn harness_named(base: &str, tweak: impl FnOnce(&mut Config)) -> Harness {
    if std::env::var("RUST_LOG").is_ok() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
    let prefix = format!("{base}-{}", ids::random_secret(8));
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
    /// log otherwise). A deploy is live as soon as traffic switched to it;
    /// this also waits for the end of its log (old instances drained).
    async fn expect(&self, d: &Deploy, status: DeployStatus) -> Deploy {
        let done = self.wait_terminal(&d.id).await;
        let follow = self.engine.deploy_logs(&d.id, true).await.unwrap().collect::<Vec<_>>();
        assert!(tokio::time::timeout(Duration::from_secs(60), follow).await.is_ok(), "the log of {} never ended", d.id);
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
    // The pulled image is pinned as one of the service's own images.
    assert_eq!(d1.image, Some(format!("{}/web:{}", h.prefix, d1.id)));
    assert!(h.deploy_log(&d1.id).await.contains(&format!("==> Pinned nginx:alpine as {}/web:", h.prefix)));
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
    // The deploy is live as soon as traffic switched; the old instances are
    // drained and removed right after.
    h.wait_until("the old instances to be removed", Duration::from_secs(30), || async {
        h.containers(&format!("ferry.deploy={}", d1.id)).is_empty()
    })
    .await;
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
    assert!(error.contains("crashed (exit code 3)"), "the exit code is reported: {error}");
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

    // Status and runtime logs (once the previous instances are drained).
    h.wait_until("the previous instances to be removed", Duration::from_secs(30), || async {
        h.engine.service_status(&svc.id).await.unwrap().instances.len() == 2
    })
    .await;
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

    // Once traffic switched to the new instances, a cancel is refused at
    // once (no waiting for the old instances to drain).
    let d = h.engine.deploy(&ok.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    h.wait_until("the traffic switch", Duration::from_secs(60), || async {
        h.deploy_log(&d.id).await.contains("==> Routing traffic")
            || h.store.require_deploy(&d.id).await.unwrap().status.is_terminal()
    })
    .await;
    let started = Instant::now();
    match h.engine.cancel_deploy(&d.id).await {
        Err(ferry_core::Error::Conflict(m)) => assert!(m.contains("too late"), "{m}"),
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    h.expect(&d, DeployStatus::Live).await;
}

// ---------------------------------------------------------------------------
// Regression tests of fixed findings (prefix `ferryfix-engine-…`).

/// `docker inspect -f <format> <object>` (empty on failure).
fn inspect(object: &str, format: &str) -> String {
    docker_cli(&["inspect", "-f", format, object]).1
}

/// The value of `key` in a container's environment.
fn container_env(container: &str, key: &str) -> Option<String> {
    let env: Vec<String> = serde_json::from_str(&inspect(container, "{{json .Config.Env}}")).unwrap_or_default();
    env.iter().find_map(|kv| kv.strip_prefix(&format!("{key}=")).map(str::to_string))
}

fn image_id(image: &str) -> String {
    docker_cli(&["image", "inspect", "-f", "{{.Id}}", image]).1
}

/// Settings and env changed without a deploy, a failed env-change deploy and
/// a failed recreate deploy of a disk service never change what the live
/// deploy's instances run: scale-ups, crash replacements and one-off jobs use
/// the live deploy's launch spec.
#[tokio::test(flavor = "multi_thread")]
async fn live_deploys_keep_their_launch_spec() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let svc = h.create_service("web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    h.store.set_env(&svc.id, "GREETING", "one").await.unwrap();
    let d1 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    h.wait_ok("web", "/", Duration::from_secs(30)).await;
    let deploy_label = format!("ferry.deploy={}", d1.id);

    // Saved, not deployed: a start command that crashes and a new env value.
    let mut changed = h.store.require_service(&svc.id).await.unwrap();
    changed.start_command = Some("exit 3".into());
    h.store.update_service(&changed).await.unwrap();
    h.store.set_env(&svc.id, "GREETING", "two").await.unwrap();
    // A scale-up starts the new instance like the others.
    h.engine.scale(&svc.id, 2).await.unwrap();
    h.wait_until("2 routed instances", Duration::from_secs(60), || async { h.upstreams("web") == 2 }).await;
    let instances = h.containers(&deploy_label);
    assert_eq!(instances.len(), 2);
    for c in &instances {
        assert_eq!(container_env(c, "GREETING").as_deref(), Some("one"), "instance {c}");
        assert!(!inspect(c, "{{json .Config.Cmd}}").contains("exit 3"), "instance {c}");
    }

    // An env change that can't be resolved fails its deploy...
    h.store.set_env(&svc.id, "BAD", "${{datastore.nope.connectionString}}").await.unwrap();
    let r = h.engine.restart(&svc.id, DeployTrigger::EnvChange).await.unwrap();
    let r = h.expect(&r, DeployStatus::DeployFailed).await;
    assert!(r.error.as_deref().is_some_and(|e| e.contains("cannot resolve")), "{r:?}");
    // ...and the live deploy still heals: lost instances come back as deployed.
    let mut args = vec!["rm", "-f"];
    args.extend(instances.iter().map(String::as_str));
    assert!(docker_cli(&args).0);
    h.wait_until("replacement instances", Duration::from_secs(60), || async {
        let now = h.containers(&deploy_label);
        now.len() == 2
            && now.iter().all(|c| !instances.contains(c) && inspect(c, "{{.State.Running}}") == "true")
            && h.upstreams("web") == 2
    })
    .await;
    h.wait_ok("web", "/", Duration::from_secs(30)).await;
    for c in h.containers(&deploy_label) {
        assert_eq!(container_env(&c, "GREETING").as_deref(), Some("one"), "instance {c}");
        assert_eq!(container_env(&c, "BAD"), None, "instance {c}");
    }
    assert_eq!(h.store.require_service(&svc.id).await.unwrap().live_deploy_id.as_deref(), Some(d1.id.as_str()));

    // A disk service: one-off jobs get the disk; a failed recreate deploy
    // (which stops the old instance first) doesn't leave the service down.
    let disk = h
        .create_service("dsk", ServiceType::WebService, |s| {
            s.image = Some("nginx:alpine".into());
            s.disk_mount_path = Some("/data".into());
        })
        .await;
    let dd1 = h.deploy_live(&disk, DeployRequest::new(DeployTrigger::Create)).await;
    let c1 = h.containers(&format!("ferry.deploy={}", dd1.id))[0].clone();
    assert!(docker_cli(&["exec", &c1, "sh", "-c", "echo kept > /data/f"]).0);
    let job = h.engine.run_job(&disk.id, Some("cat /data/f".into()), JobTrigger::Manual).await.unwrap();
    let job = h.wait_job(&job.id).await;
    let lines: Vec<LogLine> = h.engine.job_logs(&job.id, false).await.unwrap().collect().await;
    assert_eq!(job.status, JobStatus::Succeeded, "{lines:?}");
    assert!(lines.iter().any(|l| l.line == "kept"), "the job reads the service's disk: {lines:?}");
    assert!(lines.iter().any(|l| l.line.contains("Mounting the service's disk at /data")), "{lines:?}");

    let mut broken = h.store.require_service(&disk.id).await.unwrap();
    broken.start_command = Some("exit 3".into());
    h.store.update_service(&broken).await.unwrap();
    let r = h.engine.restart(&disk.id, DeployTrigger::Restart).await.unwrap();
    let r = h.expect(&r, DeployStatus::DeployFailed).await;
    assert!(h.deploy_log(&r.id).await.contains("redeployed in place"));
    h.wait_ok("dsk", "/", Duration::from_secs(60)).await;
    let back = h.containers(&format!("ferry.deploy={}", dd1.id));
    assert_eq!(back.len(), 1, "the live deploy's instance is back");
    assert!(!inspect(&back[0], "{{json .Config.Cmd}}").contains("exit 3"));
    assert_eq!(docker_cli(&["exec", &back[0], "cat", "/data/f"]).1, "kept");
}

/// Image deploys are pinned to what was pulled: a moved tag changes neither
/// the live deploy's crash replacements nor a rollback.
#[tokio::test(flavor = "multi_thread")]
async fn image_deploys_are_pinned() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    // A local-only `:latest` tag (the pull fails, the local copy is used).
    let src = format!("{}/src:latest", h.prefix);
    assert!(docker_cli(&["tag", "nginx:alpine", &src]).0);
    let svc = h.create_service("web", ServiceType::WebService, |s| s.image = Some(src.clone())).await;
    let d1 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    let pinned = format!("{}/web:{}", h.prefix, d1.id);
    assert_eq!(d1.image.as_deref(), Some(pinned.as_str()));
    assert_eq!(image_id(&pinned), image_id("nginx:alpine"));
    assert!(h.wait_ok("web", "/", Duration::from_secs(30)).await.contains("nginx"));

    // The tag moves to an image that can't serve: that deploy fails...
    assert!(docker_cli(&["tag", "busybox:stable", &src]).0);
    let d2 = h.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    h.expect(&d2, DeployStatus::DeployFailed).await;
    // ...and a crash replacement of the live deploy still runs its image.
    let before = h.containers(&format!("ferry.deploy={}", d1.id));
    assert!(docker_cli(&["rm", "-f", &before[0]]).0);
    h.wait_until("a replacement instance", Duration::from_secs(60), || async {
        let now = h.containers(&format!("ferry.deploy={}", d1.id));
        now.len() == 1 && now[0] != before[0] && inspect(&now[0], "{{.State.Running}}") == "true"
    })
    .await;
    let replacement = h.containers(&format!("ferry.deploy={}", d1.id))[0].clone();
    assert_eq!(inspect(&replacement, "{{.Config.Image}}"), pinned);
    assert!(h.wait_ok("web", "/", Duration::from_secs(30)).await.contains("nginx"));

    // Another version goes live; a rollback restores exactly the first image.
    let mut s = h.store.require_service(&svc.id).await.unwrap();
    s.start_command = Some("exec httpd -f -p 8080 -h /etc".into());
    s.port = Some(8080);
    h.store.update_service(&s).await.unwrap();
    let d3 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Manual)).await;
    assert_eq!(image_id(d3.image.as_deref().unwrap()), image_id("busybox:stable"));
    s.start_command = None;
    s.port = None;
    h.store.update_service(&s).await.unwrap();
    let d4 = h.engine.rollback(&svc.id, &d1.id).await.unwrap();
    let d4 = h.expect(&d4, DeployStatus::Live).await;
    assert_eq!(d4.image.as_deref(), Some(pinned.as_str()));
    assert!(h.wait_ok("web", "/", Duration::from_secs(30)).await.contains("nginx"));

    // Deleting the service removes only Ferry's tags, never the pulled images.
    h.engine.delete_service(&svc.id).await.unwrap();
    assert!(h.own_images("web").is_empty(), "{:?}", h.own_images("web"));
    assert!(docker_cli(&["image", "inspect", "nginx:alpine"]).0);
    assert!(docker_cli(&["image", "inspect", "busybox:stable"]).0);
}

/// A service referencing another one's port while both deploy for the first
/// time (a blueprint apply) waits for it instead of failing.
#[tokio::test(flavor = "multi_thread")]
async fn port_references_wait_for_the_first_deploy() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let api = h
        .create_service("api", ServiceType::PrivateService, |s| {
            s.image = Some("python:3.12-alpine".into());
            s.start_command = Some("sleep 6; exec python -m http.server $PORT".into());
        })
        .await;
    h.store.set_env(&api.id, "PORT", "9000").await.unwrap();
    let web = h.create_service("web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    h.store.set_env(&web.id, "INTERNAL_HOSTPORT", "${{service.api.hostport}}").await.unwrap();

    let da = h.engine.deploy(&api.id, DeployRequest::new(DeployTrigger::Blueprint)).await.unwrap();
    let dw = h.engine.deploy(&web.id, DeployRequest::new(DeployTrigger::Blueprint)).await.unwrap();
    let dw = h.expect(&dw, DeployStatus::Live).await;
    let da = h.expect(&da, DeployStatus::Live).await;
    assert_eq!(da.port, Some(9000));
    let log = h.deploy_log(&dw.id).await;
    assert!(log.contains("Waiting for service 'api'"), "{log}");
    let c = h.containers(&format!("ferry.deploy={}", dw.id))[0].clone();
    assert_eq!(container_env(&c, "INTERNAL_HOSTPORT").as_deref(), Some("api:9000"));
}

/// Datastores never adopt a volume they did not create, and a stopped
/// datastore container that can't start again (its host port was taken) is
/// recreated with the same volume.
#[tokio::test(flavor = "multi_thread")]
async fn datastores_refuse_foreign_volumes_and_recover() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let volume = format!("{}-ds-stale-data", h.prefix);
    assert!(docker_cli(&["volume", "create", &volume]).0);
    let stale = Datastore::new("stale", DatastoreKind::Postgres);
    h.store.create_datastore(&stale).await.unwrap();
    h.engine.provision_datastore(&stale.id).await.unwrap();
    h.wait_until("the datastore to fail", Duration::from_secs(60), || async {
        h.store.require_datastore(&stale.id).await.unwrap().status == DatastoreStatus::Failed
    })
    .await;
    let error = h.store.require_datastore(&stale.id).await.unwrap().error.unwrap_or_default();
    assert!(error.contains("already exists") && error.contains(&volume), "{error}");
    assert!(h.containers(&format!("ferry.datastore={}", stale.id)).is_empty());
    h.engine.delete_datastore(&stale.id).await.unwrap();
    // Deleting the datastore never deletes a volume Ferry did not create for it.
    assert!(h.store.find_datastore(&stale.id).await.unwrap().is_none());
    assert!(docker_cli(&["volume", "inspect", &volume]).0, "the foreign volume {volume} was deleted");

    let cache = Datastore::new("cache", DatastoreKind::Redis);
    h.store.create_datastore(&cache).await.unwrap();
    h.engine.provision_datastore(&cache.id).await.unwrap();
    let cache = h.wait_datastore(&cache.id, Duration::from_secs(120)).await;
    let container = format!("{}-ds-cache", h.prefix);
    let auth = format!("REDISCLI_AUTH={}", cache.password);
    let redis = |args: &[&str]| {
        let mut all = vec!["exec", "-e", auth.as_str(), container.as_str(), "redis-cli"];
        all.extend_from_slice(args);
        docker_cli(&all).1
    };
    assert_eq!(redis(&["set", "k", "kept"]), "OK");
    let port = cache.host_port.unwrap();
    assert!(docker_cli(&["stop", &container]).0);
    let blocker = std::net::TcpListener::bind(("127.0.0.1", port)).expect("the old host port is free");
    h.wait_until("the datastore to be recreated on another port", Duration::from_secs(90), || async {
        let ds = h.store.require_datastore(&cache.id).await.unwrap();
        ds.status == DatastoreStatus::Available
            && ds.host_port.is_some_and(|p| p != port)
            && inspect(&container, "{{.State.Running}}") == "true"
    })
    .await;
    drop(blocker);
    assert_eq!(redis(&["get", "k"]), "kept", "the volume keeps the data");
}

/// Shutdown stops running jobs (with their grace period) and records them
/// before `stopped()` resolves.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_stops_running_jobs() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let svc = h
        .create_service("bg", ServiceType::BackgroundWorker, |s| {
            s.image = Some("busybox:stable".into());
            s.start_command = Some("exec sleep 3600".into());
        })
        .await;
    h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    let job = h.engine.run_job(&svc.id, Some("echo started; sleep 300".into()), JobTrigger::Manual).await.unwrap();
    h.wait_until("the job to run", Duration::from_secs(60), || async {
        h.store.require_job_run(&job.id).await.unwrap().status == JobStatus::Running
            && !h.containers(&format!("ferry.job={}", job.id)).is_empty()
    })
    .await;

    h.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(40), h.engine.stopped()).await.expect("the engine stops");
    let j = h.store.require_job_run(&job.id).await.unwrap();
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(j.error.as_deref(), Some("interrupted by server shutdown"));
    assert!(h.containers(&format!("ferry.job={}", job.id)).is_empty(), "the job container is removed");
    let lines: Vec<LogLine> = h.engine.job_logs(&job.id, true).await.unwrap().collect().await;
    assert!(lines.iter().any(|l| l.line == "started"), "{lines:?}");
    assert!(lines.last().is_some_and(|l| l.line.contains("interrupted by server shutdown")), "{lines:?}");
}

/// After a crash, a new server serves the live deploy as soon as the engine
/// has started: routes are installed before stale instances (which may take
/// their whole stop grace period) are removed in the background.
#[tokio::test(flavor = "multi_thread")]
async fn boot_routes_the_live_deploy_before_slow_cleanup() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let svc = h.create_service("web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    // Left behind by a crash: an instance of another deploy that ignores
    // SIGTERM (so stopping it takes the full grace period).
    let stale = format!("{}-web-stale", h.prefix);
    let labels = [
        format!("ferry.instance={}", h.prefix),
        "ferry.managed=true".to_string(),
        "ferry.role=service".to_string(),
        format!("ferry.service={}", svc.id),
        "ferry.deploy=dep-stale".to_string(),
    ];
    let mut args = vec!["run", "-d", "--name", stale.as_str()];
    for l in &labels {
        args.extend(["--label", l.as_str()]);
    }
    args.extend(["busybox:stable", "sleep", "3600"]);
    assert!(docker_cli(&args).0);

    // This server stops; a new one boots on the same data.
    h.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(40), h.engine.stopped()).await.expect("the engine stops");
    let routes = RouteTable::new();
    let docker = Docker::connect().await.unwrap();
    let builder = Builder::new(h.config.builds_dir(), h.config.repos_dir(), h.config.docker_bin.clone());
    let engine = FerryEngine::new(h.config.clone(), h.store.clone(), docker, builder, routes.clone());
    let shutdown = CancellationToken::new();
    let started = Instant::now();
    engine.start(shutdown.clone()).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "boot waited for the cleanup: {:?}", started.elapsed());
    // Routed right away, to an instance that serves.
    let upstream = match routes.resolve(&h.host("web")) {
        ferry_proxy::Resolution::Upstream(addr) => addr,
        other => panic!("no upstream right after boot: {other:?}"),
    };
    let resp = h.http.get(format!("http://{upstream}/")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    // The stale instance goes in the background.
    h.wait_until("the stale instance to be removed", Duration::from_secs(60), || async {
        !docker_cli(&["inspect", &stale]).0
    })
    .await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(40), engine.stopped()).await.expect("the new engine stops");
}

/// Docker restarts an instance on a new host port: the routes follow within
/// a couple of seconds, not at the next reconcile pass.
#[tokio::test(flavor = "multi_thread")]
async fn routes_follow_restarted_instances() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let svc = h.create_service("web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let d = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    h.wait_ok("web", "/", Duration::from_secs(30)).await;
    let c = h.containers(&format!("ferry.deploy={}", d.id))[0].clone();
    let port_of = |c: &str| inspect(c, "{{(index (index .NetworkSettings.Ports \"80/tcp\") 0).HostPort}}");
    let before = port_of(&c);
    assert!(docker_cli(&["restart", "-t", "1", &c]).0);
    let restarted = Instant::now();
    let after = port_of(&c);
    let expected: SocketAddr = format!("127.0.0.1:{after}").parse().unwrap();
    h.wait_until("the route to follow the new port", Duration::from_secs(15), || async {
        let host = h.host("web");
        h.routes.snapshot().into_iter().any(|r| r.host == host && r.upstreams == vec![expected])
    })
    .await;
    assert!(
        restarted.elapsed() < Duration::from_secs(6),
        "routes took {:?} to follow port {before} → {after}",
        restarted.elapsed()
    );
    h.wait_ok("web", "/", Duration::from_secs(5)).await;
}

/// `docker run -d` a container labelled like an instance of `deploy_id`
/// that ignores SIGTERM (a graceful stop takes the whole grace period).
fn fake_instance(h: &Harness, name: &str, service_id: &str, deploy_id: &str) {
    let labels = [
        format!("ferry.instance={}", h.prefix),
        "ferry.managed=true".to_string(),
        "ferry.role=service".to_string(),
        format!("ferry.service={service_id}"),
        format!("ferry.deploy={deploy_id}"),
    ];
    let mut args = vec!["run", "-d", "--name", name];
    for l in &labels {
        args.extend(["--label", l.as_str()]);
    }
    args.extend(["busybox:stable", "sleep", "3600"]);
    assert!(docker_cli(&args).0, "docker run {name}");
}

/// kill -9 while a recreate (disk) deploy health-checks its new instance:
/// the old instance is already gone, the new one never went live. At boot
/// that instance is removed at once (no stop grace period) and the live
/// deploy's instance is back within seconds.
#[tokio::test(flavor = "multi_thread")]
async fn boot_removes_the_instance_of_an_interrupted_recreate_at_once() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let svc = h
        .create_service("dsk", ServiceType::WebService, |s| {
            s.image = Some("nginx:alpine".into());
            s.disk_mount_path = Some("/data".into());
        })
        .await;
    let d1 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    h.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(40), h.engine.stopped()).await.expect("the engine stops");

    // What the crash left: the live instance stopped and removed, the new
    // one running and its deploy still `deploying`.
    let mut d2 = Deploy::new(
        &svc.id,
        DeployTrigger::Restart,
        DeploySource::Reuse { image: d1.image.clone().unwrap(), from_deploy: Some(d1.id.clone()) },
    );
    d2.status = DeployStatus::Deploying;
    d2.image = d1.image.clone();
    h.store.create_deploy(&d2).await.unwrap();
    for c in h.containers(&format!("ferry.deploy={}", d1.id)) {
        assert!(docker_cli(&["rm", "-f", &c]).0);
    }
    let stale = format!("{}-dsk-interrupted", h.prefix);
    fake_instance(&h, &stale, &svc.id, &d2.id);

    // A new server boots on the same data.
    let routes = RouteTable::new();
    let docker = Docker::connect().await.unwrap();
    let builder = Builder::new(h.config.builds_dir(), h.config.repos_dir(), h.config.docker_bin.clone());
    let engine = FerryEngine::new(h.config.clone(), h.store.clone(), docker, builder, routes.clone());
    let shutdown = CancellationToken::new();
    let started = Instant::now();
    engine.start(shutdown.clone()).await.unwrap();
    let host = h.host("dsk");
    h.wait_until("the live instance to serve again", Duration::from_secs(40), || async {
        match routes.resolve(&host) {
            ferry_proxy::Resolution::Upstream(addr) => {
                h.http.get(format!("http://{addr}/")).send().await.is_ok_and(|r| r.status().as_u16() == 200)
            }
            _ => false,
        }
    })
    .await;
    let outage = started.elapsed();
    assert!(outage < Duration::from_secs(8), "the live instance took {outage:?} to come back");
    assert!(!docker_cli(&["inspect", &stale]).0, "the interrupted deploy's instance is removed");
    let d2 = h.store.require_deploy(&d2.id).await.unwrap();
    assert_eq!(d2.status, DeployStatus::DeployFailed);
    assert_eq!(d2.error.as_deref(), Some("interrupted by server restart"));
    assert_eq!(h.containers(&format!("ferry.deploy={}", d1.id)).len(), 1);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(40), engine.stopped()).await.expect("the new engine stops");
}

/// The lines of a job's log so far (or all of them once it is finished).
async fn job_lines(h: &Harness, id: &str, follow: bool) -> Vec<String> {
    let stream = h.engine.job_logs(id, follow).await.unwrap().collect::<Vec<LogLine>>();
    let lines = tokio::time::timeout(Duration::from_secs(60), stream).await.expect("the job log ends");
    lines.into_iter().map(|l| l.line).collect()
}

/// A job is canceled with its container's grace period; suspending a
/// service stops its running jobs.
#[tokio::test(flavor = "multi_thread")]
async fn jobs_are_canceled_and_suspend_stops_them() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let svc = h
        .create_service("bg", ServiceType::BackgroundWorker, |s| {
            s.image = Some("busybox:stable".into());
            s.start_command = Some("exec sleep 3600".into());
        })
        .await;
    h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    // Runs until SIGTERM, then says so and exits (after the current `sleep 1`).
    let command = "trap 'echo got-term; exit 143' TERM; echo started; while true; do sleep 1; done";
    async fn start(h: &Harness, service_id: &str, command: &str) -> ferry_core::JobRun {
        let job = h.engine.run_job(service_id, Some(command.into()), JobTrigger::Manual).await.unwrap();
        h.wait_until("the job to run", Duration::from_secs(60), || async {
            h.store.require_job_run(&job.id).await.unwrap().status == JobStatus::Running
                && job_lines(h, &job.id, false).await.iter().any(|l| l == "started")
        })
        .await;
        job
    }

    let job = start(&h, &svc.id, command).await;
    let asked = Instant::now();
    let canceled = h.engine.cancel_job(&job.id).await.unwrap();
    assert!(asked.elapsed() < Duration::from_secs(9), "graceful stop: {:?}", asked.elapsed());
    assert_eq!(canceled.status, JobStatus::Canceled, "{canceled:?}");
    assert_eq!(canceled.error.as_deref(), Some("canceled by user"));
    assert_eq!(canceled.exit_code, Some(143));
    assert!(canceled.finished_at.is_some());
    assert!(h.containers(&format!("ferry.job={}", job.id)).is_empty(), "the job container is removed");
    let lines = job_lines(&h, &job.id, true).await;
    assert!(lines.iter().any(|l| l == "got-term"), "the container got its grace period: {lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("==> Job canceled: canceled by user"), "{lines:?}");
    match h.engine.cancel_job(&job.id).await {
        Err(ferry_core::Error::Conflict(m)) => assert!(m.contains("already finished"), "{m}"),
        other => panic!("expected a conflict, got {other:?}"),
    }

    // Suspend stops the running job before it returns.
    let job = start(&h, &svc.id, command).await;
    h.engine.suspend(&svc.id).await.unwrap();
    let j = h.store.require_job_run(&job.id).await.unwrap();
    assert_eq!(j.status, JobStatus::Canceled, "{j:?}");
    assert_eq!(j.error.as_deref(), Some("service suspended"));
    assert!(h.containers(&format!("ferry.job={}", job.id)).is_empty());
    assert!(job_lines(&h, &job.id, true).await.iter().any(|l| l == "got-term"));
    assert!(matches!(
        h.engine.run_job(&svc.id, Some("true".into()), JobTrigger::Manual).await,
        Err(ferry_core::Error::Conflict(_))
    ));
}

/// Missing images: a rollback says the image was cleaned up by retention, a
/// restart that it was removed from Docker; resuming a suspended service
/// whose live image is gone is refused (it stays suspended) and a manual
/// deploy resumes it.
#[tokio::test(flavor = "multi_thread")]
async fn missing_images_are_explained_and_a_manual_deploy_resumes() {
    require_e2e!();
    let h = fix_harness(|c| c.keep_images = 1).await;
    let svc = h.create_service("web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let d1 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Create)).await;
    let d2 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Manual)).await;
    let d1_image = d1.image.clone().unwrap();
    h.wait_until("retention to delete the first image", Duration::from_secs(30), || async {
        !docker_cli(&["image", "inspect", &d1_image]).0
    })
    .await;
    match h.engine.rollback(&svc.id, &d1.id).await {
        Err(ferry_core::Error::Invalid(m)) => assert!(m.contains("image retention"), "{m}"),
        other => panic!("expected Invalid, got {other:?}"),
    }

    // The live image removed behind Ferry's back (its instance keeps running).
    assert!(docker_cli(&["rmi", "-f", d2.image.as_deref().unwrap()]).0);
    match h.engine.restart(&svc.id, DeployTrigger::Restart).await {
        Err(ferry_core::Error::Conflict(m)) => {
            assert!(m.contains("removed from Docker") && m.contains(&d2.id), "{m}");
        }
        other => panic!("expected Conflict, got {other:?}"),
    }

    h.engine.suspend(&svc.id).await.unwrap();
    match h.engine.resume(&svc.id).await {
        Err(ferry_core::Error::Conflict(m)) => assert!(
            m.contains(&format!("the image of the live deploy {} no longer exists — deploy again", d2.id)),
            "{m}"
        ),
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert!(h.store.require_service(&svc.id).await.unwrap().suspended, "still suspended");
    assert_eq!(h.get("web", "/").await.unwrap().0, 503);
    // Automatic deploys don't resume it; a manual one does, once live.
    assert!(matches!(
        h.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::DeployHook)).await,
        Err(ferry_core::Error::Conflict(_))
    ));
    let d3 = h.deploy_live(&svc, DeployRequest::new(DeployTrigger::Manual)).await;
    assert!(h.deploy_log(&d3.id).await.contains("==> Resumed 'web'"), "{}", h.deploy_log(&d3.id).await);
    assert!(!h.store.require_service(&svc.id).await.unwrap().suspended);
    h.wait_ok("web", "/", Duration::from_secs(30)).await;
    assert_eq!(h.engine.service_status(&svc.id).await.unwrap().state, ServiceState::Live);
}

/// A restart of a live cron job (the API queues one when its start command
/// changes) snapshots the current start command, starts no container, and
/// the next runs use the new command; the schedule is always the current one.
#[tokio::test(flavor = "multi_thread")]
async fn cron_restarts_snapshot_the_current_settings() {
    require_e2e!();
    let h = fix_harness(|_| {}).await;
    let cron = h
        .create_service("tick", ServiceType::CronJob, |s| {
            s.image = Some("busybox:stable".into());
            s.schedule = Some("0 0 * * *".into());
            s.start_command = Some("echo first command".into());
        })
        .await;
    let d1 = h.deploy_live(&cron, DeployRequest::new(DeployTrigger::Create)).await;
    let mut changed = h.store.require_service(&cron.id).await.unwrap();
    changed.start_command = Some("echo second command".into());
    changed.schedule = Some("30 3 * * *".into());
    h.store.update_service(&changed).await.unwrap();

    // Saved, not applied yet: runs use the live deploy's command.
    let job = h.engine.run_job(&cron.id, None, JobTrigger::Manual).await.unwrap();
    assert_eq!(h.wait_job(&job.id).await.status, JobStatus::Succeeded);
    assert!(job_lines(&h, &job.id, true).await.iter().any(|l| l == "first command"));

    let d2 = h.engine.restart(&cron.id, DeployTrigger::Restart).await.unwrap();
    let d2 = h.expect(&d2, DeployStatus::Live).await;
    assert!(h.containers(&format!("ferry.deploy={}", d2.id)).is_empty(), "a cron restart starts no container");
    assert_eq!(h.store.require_deploy(&d1.id).await.unwrap().status, DeployStatus::Deactivated);
    let log = h.deploy_log(&d2.id).await;
    assert!(log.contains("==> Your cron job is live") && log.contains("30 3 * * *"), "{log}");

    let job = h.engine.run_job(&cron.id, None, JobTrigger::Manual).await.unwrap();
    assert_eq!(h.wait_job(&job.id).await.status, JobStatus::Succeeded);
    let lines = job_lines(&h, &job.id, true).await;
    assert!(lines.iter().any(|l| l == "second command"), "{lines:?}");
}
