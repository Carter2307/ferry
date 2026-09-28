//! Shared test harness: an in-memory store, a recording `MockEngine` and
//! helpers to drive the router with `tower::ServiceExt::oneshot`.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use ferry_api::{AppState, router};
use ferry_core::dto::{InstanceStatus, RuntimeStatus};
use ferry_core::{
    CancellationToken, Config, Deploy, DeployRequest, DeploySource, DeployStatus, DeployTrigger, Engine, Error, JobRun,
    JobStatus, JobTrigger, LogLine, LogOptions, LogStream, Result, ServiceState, Store, compute_service_state,
};
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

pub const TOKEN: &str = "test-token-0123456789";

/// Records every call; mutates the store the way the real engine would.
pub struct MockEngine {
    pub store: Store,
    pub calls: Mutex<Vec<String>>,
    pub fail_provision: AtomicBool,
    pub fail_restart: AtomicBool,
    pub fail_scale: AtomicBool,
    pub fail_update_limits: AtomicBool,
    /// Running instances `service_status` reports (default: all desired).
    pub running: Mutex<Option<u32>>,
}

impl MockEngine {
    pub fn new(store: Store) -> Self {
        MockEngine {
            store,
            calls: Mutex::new(Vec::new()),
            fail_provision: AtomicBool::new(false),
            fail_restart: AtomicBool::new(false),
            fail_scale: AtomicBool::new(false),
            fail_update_limits: AtomicBool::new(false),
            running: Mutex::new(None),
        }
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Calls starting with `prefix` (e.g. "deploy ").
    pub fn calls_with(&self, prefix: &str) -> Vec<String> {
        self.calls().into_iter().filter(|c| c.starts_with(prefix)).collect()
    }

    pub fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }

    fn lines(prefix: &str) -> Vec<LogLine> {
        vec![LogLine::system(format!("==> {prefix} one")), LogLine::system(format!("==> {prefix} two"))]
    }
}

#[async_trait]
impl Engine for MockEngine {
    async fn deploy(&self, service_id: &str, req: DeployRequest) -> Result<Deploy> {
        self.record(format!(
            "deploy {service_id} {} commit={} clear_cache={} source={}",
            req.trigger,
            req.commit.as_deref().unwrap_or("-"),
            req.clear_cache,
            req.source.as_ref().map(|s| serde_json::to_string(s).unwrap()).unwrap_or_else(|| "-".into()),
        ));
        let svc = self.store.get_service(service_id).await?.ok_or_else(|| Error::not_found("service", service_id))?;
        if svc.suspended {
            return Err(Error::conflict(format!("service '{}' is suspended", svc.name)));
        }
        let source = match req.source {
            Some(s) => s,
            None => {
                if let Some(image) = &svc.image {
                    DeploySource::Image { image: image.clone() }
                } else if let Some(repo) = &svc.repo_url {
                    DeploySource::Git { repo_url: repo.clone(), branch: svc.branch.clone(), commit: req.commit.clone() }
                } else {
                    return Err(Error::invalid("no source: upload code with `ferry up`"));
                }
            }
        };
        let mut d = Deploy::new(&svc.id, req.trigger, source);
        d.commit_sha = req.commit;
        self.store.create_deploy(&d).await?;
        Ok(d)
    }

    async fn cancel_deploy(&self, deploy_id: &str) -> Result<Deploy> {
        self.record(format!("cancel {deploy_id}"));
        let d = self.store.require_deploy(deploy_id).await?;
        if d.status.is_terminal() {
            return Err(Error::conflict(format!("deploy {deploy_id} already finished")));
        }
        self.store.set_deploy_status(deploy_id, DeployStatus::Canceled, None).await
    }

    async fn rollback(&self, service_id: &str, deploy_id: &str) -> Result<Deploy> {
        self.record(format!("rollback {service_id} {deploy_id}"));
        let d = Deploy::new(
            service_id,
            DeployTrigger::Rollback,
            DeploySource::Reuse { image: "img".into(), from_deploy: Some(deploy_id.into()) },
        );
        self.store.create_deploy(&d).await?;
        Ok(d)
    }

    async fn restart(&self, service_id: &str, trigger: DeployTrigger) -> Result<Deploy> {
        self.record(format!("restart {service_id} {trigger}"));
        if self.fail_restart.load(Ordering::SeqCst) {
            return Err(Error::docker("restart exploded"));
        }
        let svc = self.store.require_service(service_id).await?;
        // Like the real engine: reuse the live image, or queue behind a
        // first deploy that is still in flight.
        let in_flight = self.store.active_deploys().await?.into_iter().any(|d| d.service_id == service_id);
        let from = match (&svc.live_deploy_id, in_flight) {
            (Some(live), _) => Some(live.clone()),
            (None, true) => None,
            (None, false) => return Err(Error::conflict("no live deploy")),
        };
        let d = Deploy::new(service_id, trigger, DeploySource::Reuse { image: "img".into(), from_deploy: from });
        self.store.create_deploy(&d).await?;
        Ok(d)
    }

    async fn suspend(&self, service_id: &str) -> Result<()> {
        self.record(format!("suspend {service_id}"));
        self.store.set_suspended(service_id, true).await
    }

    async fn resume(&self, service_id: &str) -> Result<()> {
        self.record(format!("resume {service_id}"));
        self.store.set_suspended(service_id, false).await
    }

    async fn scale(&self, service_id: &str, instances: u32) -> Result<()> {
        self.record(format!("scale {service_id} {instances}"));
        if self.fail_scale.load(Ordering::SeqCst) {
            return Err(Error::docker("scale exploded"));
        }
        self.store.set_instances(service_id, instances).await
    }

    async fn delete_service(&self, service_id: &str) -> Result<()> {
        self.record(format!("delete_service {service_id}"));
        self.store.delete_service(service_id).await
    }

    async fn refresh_routes(&self, service_id: &str) -> Result<()> {
        self.record(format!("refresh_routes {service_id}"));
        Ok(())
    }

    async fn service_status(&self, service_id: &str) -> Result<RuntimeStatus> {
        self.record(format!("service_status {service_id}"));
        let svc = self.store.require_service(service_id).await?;
        let latest = self.store.latest_deploy(service_id).await?;
        let desired = svc.desired_instances().max(1);
        let running = self.running.lock().unwrap().unwrap_or(desired);
        let instances = (0..desired)
            .map(|i| InstanceStatus {
                container_id: format!("c0ffee{i}"),
                name: format!("ferry-web-x{i}"),
                deploy_id: svc.live_deploy_id.clone(),
                state: if i < running { "running" } else { "exited" }.into(),
                host_port: Some(32768),
                started_at: None,
                restart_count: Some(0),
                cpu_percent: Some(1.5),
                memory_bytes: Some(1024),
                memory_limit_bytes: None,
                cpu_limit: None,
                oom_killed: false,
                exit_code: None,
            })
            .collect();
        Ok(RuntimeStatus {
            service_id: svc.id.clone(),
            state: compute_service_state(&svc, latest.as_ref(), Some(running)),
            desired_instances: svc.desired_instances(),
            instances,
        })
    }

    async fn service_logs(&self, service_id: &str, opts: LogOptions) -> Result<LogStream> {
        self.record(format!("service_logs {service_id} follow={} tail={:?}", opts.follow, opts.tail));
        let lines = futures::stream::iter(Self::lines("runtime").into_iter().map(|l| l.with_instance("abc123")));
        if opts.follow {
            use futures::StreamExt;
            Ok(Box::pin(lines.chain(futures::stream::pending())))
        } else {
            Ok(Box::pin(lines))
        }
    }

    async fn deploy_logs(&self, deploy_id: &str, follow: bool) -> Result<LogStream> {
        self.record(format!("deploy_logs {deploy_id} follow={follow}"));
        Ok(Box::pin(futures::stream::iter(Self::lines("build"))))
    }

    async fn run_job(&self, service_id: &str, command: Option<String>, trigger: JobTrigger) -> Result<JobRun> {
        self.record(format!("run_job {service_id} {} {trigger}", command.as_deref().unwrap_or("-")));
        let j = JobRun::new(service_id, trigger, command);
        self.store.create_job_run(&j).await?;
        Ok(j)
    }

    async fn cancel_job(&self, job_id: &str) -> Result<JobRun> {
        self.record(format!("cancel_job {job_id}"));
        let mut j = self.store.require_job_run(job_id).await?;
        if j.status.is_terminal() {
            return Err(Error::conflict(format!("job {job_id} already finished")));
        }
        j.status = JobStatus::Canceled;
        j.finished_at = Some(chrono::Utc::now());
        self.store.update_job_run(&j).await?;
        Ok(j)
    }

    async fn job_logs(&self, job_id: &str, follow: bool) -> Result<LogStream> {
        self.record(format!("job_logs {job_id} follow={follow}"));
        Ok(Box::pin(futures::stream::iter(Self::lines("job"))))
    }

    async fn provision_datastore(&self, datastore_id: &str) -> Result<()> {
        self.record(format!("provision {datastore_id}"));
        if self.fail_provision.load(Ordering::SeqCst) {
            return Err(Error::docker("daemon unreachable"));
        }
        let mut ds = self.store.require_datastore(datastore_id).await?;
        ds.host_port = Some(15432);
        self.store.update_datastore(&ds).await?;
        Ok(())
    }

    async fn delete_datastore(&self, datastore_id: &str) -> Result<()> {
        self.record(format!("delete_datastore {datastore_id}"));
        self.store.delete_datastore(datastore_id).await
    }

    /// Records the stored limits it would apply: `update_limits ID MEMORY CPUS`
    /// (`-` = the server default).
    async fn update_datastore_limits(&self, datastore_id: &str) -> Result<()> {
        let ds = self.store.require_datastore(datastore_id).await?;
        self.record(format!(
            "update_limits {datastore_id} {} {}",
            ds.memory_limit_mb.map_or_else(|| "-".to_string(), |m| m.to_string()),
            ds.cpu_limit.map_or_else(|| "-".to_string(), |c| c.to_string()),
        ));
        if self.fail_update_limits.load(Ordering::SeqCst) {
            return Err(Error::docker("docker update exploded"));
        }
        Ok(())
    }
}

/// A router over an in-memory store and a mock engine.
pub struct TestApp {
    pub router: Router,
    pub store: Store,
    pub engine: Arc<MockEngine>,
    pub config: Arc<Config>,
    pub shutdown: CancellationToken,
    pub dir: tempfile::TempDir,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Error code of an `ApiErrorBody`.
    pub fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_default().to_string()
    }
}

impl TestApp {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(f: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config =
            Config { data_dir: dir.path().to_path_buf(), api_token: TOKEN.to_string(), ..Config::default() };
        f(&mut config);
        let config = Arc::new(config);
        let store = Store::open_in_memory().await.unwrap();
        let engine = Arc::new(MockEngine::new(store.clone()));
        let shutdown = CancellationToken::new();
        let router = router(AppState {
            config: config.clone(),
            store: store.clone(),
            engine: engine.clone() as Arc<dyn Engine>,
            docker_version: Some("27.0.0".into()),
            docker_cpus: Some(4),
            docker_memory_bytes: Some(8 << 30),
            shutdown: shutdown.clone(),
        });
        TestApp { router, store, engine, config, shutdown, dir }
    }

    pub async fn send(&self, req: Request<Body>) -> Resp {
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
        Resp { status, headers, body }
    }

    /// Authenticated JSON request.
    pub async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> Resp {
        let mut b = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {TOKEN}"));
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(serde_json::to_vec(&v).unwrap())
            }
            None => Body::empty(),
        };
        self.send(b.body(body).unwrap()).await
    }

    pub async fn get(&self, uri: &str) -> Resp {
        self.call(Method::GET, uri, None).await
    }

    pub async fn post(&self, uri: &str, body: Value) -> Resp {
        self.call(Method::POST, uri, Some(body)).await
    }

    pub async fn patch(&self, uri: &str, body: Value) -> Resp {
        self.call(Method::PATCH, uri, Some(body)).await
    }

    pub async fn put(&self, uri: &str, body: Value) -> Resp {
        self.call(Method::PUT, uri, Some(body)).await
    }

    pub async fn delete(&self, uri: &str) -> Resp {
        self.call(Method::DELETE, uri, None).await
    }

    /// Create a service through the API and return its JSON view.
    pub async fn create_service(&self, body: Value) -> Value {
        let r = self.post("/api/v1/services", body).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
        r.json()
    }

    /// Mark a service live (as the engine would after a successful deploy).
    pub async fn make_live(&self, service: &str, port: Option<u16>) -> Deploy {
        let svc = self.store.require_service(service).await.unwrap();
        let mut d = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        d.status = DeployStatus::Live;
        d.port = port;
        d.image = Some("x".into());
        self.store.create_deploy(&d).await.unwrap();
        self.store.set_live_deploy(&svc.id, Some(&d.id)).await.unwrap();
        d
    }
}

/// State string of a service view.
pub fn state_of(v: &Value) -> ServiceState {
    serde_json::from_value(v["state"].clone()).unwrap()
}
