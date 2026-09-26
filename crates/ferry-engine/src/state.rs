//! The engine's shared state: dependencies plus in-memory bookkeeping of
//! running work (deploy workers, active deploys, jobs, provisioning).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use ferry_build::Builder;
use ferry_core::{CancellationToken, Config, Naming, Store};
use ferry_docker::Docker;
use ferry_proxy::RouteTable;
use tokio::sync::{Mutex as AsyncMutex, Notify, Semaphore, watch};
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;

use crate::logs::LogHub;
use crate::util::{KeyedLocks, lock};

/// Everything the engine's tasks share. Always behind an `Arc`.
pub(crate) struct Inner {
    pub config: Arc<Config>,
    pub store: Store,
    pub docker: Docker,
    pub builder: Builder,
    pub routes: RouteTable,
    pub naming: Naming,
    pub logs: LogHub,
    /// Client for HTTP health checks (no proxies, no redirects).
    pub http: std::result::Result<reqwest::Client, String>,
    /// Bounds concurrent image builds (`config.build_concurrency`).
    pub build_slots: Arc<Semaphore>,
    /// Serializes changes to the deploy queue: enqueue + supersede, cancel
    /// of queued deploys, worker claims and worker exit.
    pub queue_lock: AsyncMutex<()>,
    /// Held by whoever changes a service's containers (the deploy's
    /// deploying phase, a reconcile of that service, scale, suspend, delete).
    pub service_locks: KeyedLocks,
    /// Held while a datastore container is provisioned / deleted.
    pub datastore_locks: KeyedLocks,
    /// Serializes full reconcile passes.
    pub reconcile_lock: AsyncMutex<()>,
    /// Wakes the reconcile loop early.
    pub reconcile_wake: Notify,
    /// Cancelled when the server shuts down.
    pub shutdown: CancellationToken,
    /// Every background task of the engine (workers, jobs, loops, cleanup),
    /// so that shutdown can wait for them (`FerryEngine::stopped`).
    pub tasks: TaskTracker,
    pub started: AtomicBool,
    pub worker_generation: AtomicU64,
    pub rt: StdMutex<Runtime>,
}

/// In-memory bookkeeping (never held across an `.await`).
#[derive(Default)]
pub(crate) struct Runtime {
    /// Deploy worker per service id.
    pub workers: HashMap<String, Worker>,
    /// Claimed (building / deploying) deploys by deploy id.
    pub deploys: HashMap<String, ActiveDeploy>,
    /// Options of queued deploys that the deploy row doesn't carry.
    pub deploy_options: HashMap<String, DeployOptions>,
    /// Services whose deploy is in the deploying phase.
    pub deploying: HashSet<String>,
    /// Running job runs by job id.
    pub jobs: HashMap<String, RunningJob>,
    /// Datastores being provisioned (id → cancel token).
    pub provisioning: HashMap<String, CancellationToken>,
    pub deleting_services: HashSet<String>,
    pub deleting_datastores: HashSet<String>,
    /// Services with a route warm-up task running.
    pub warmups: HashSet<String>,
    /// Failed datastores: when the reconciler may retry them, and the
    /// current back-off.
    pub datastore_retries: HashMap<String, (std::time::Instant, Duration)>,
    /// Deploys waiting for services they reference (service id → the ids of
    /// the services it waits for), to detect circular waits.
    pub reference_waits: HashMap<String, HashSet<String>>,
}

/// A service's deploy worker.
pub(crate) struct Worker {
    pub generation: u64,
    pub wake: Arc<Notify>,
    pub handle: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DeployOptions {
    pub clear_cache: bool,
}

/// A deploy a worker has claimed.
#[derive(Clone)]
pub(crate) struct ActiveDeploy {
    pub service_id: String,
    pub cancel: CancellationToken,
    control: Arc<StdMutex<Control>>,
    /// Becomes `true` once the deploy reached a terminal status.
    pub done: watch::Receiver<bool>,
}

#[derive(Debug, Default)]
struct Control {
    /// Why it was cancelled (becomes the deploy's error).
    reason: Option<String>,
    /// Traffic was switched to the deploy: it can no longer be cancelled.
    swapped: bool,
}

impl ActiveDeploy {
    pub fn new(service_id: &str, cancel: CancellationToken, done: watch::Receiver<bool>) -> Self {
        ActiveDeploy { service_id: service_id.to_string(), cancel, control: Arc::default(), done }
    }

    /// Ask the deploy to stop. Returns `false` (and changes nothing) once
    /// traffic was switched to it: from then on it always goes live.
    pub fn request_cancel(&self, reason: &str) -> bool {
        let mut control = lock(&self.control);
        if control.swapped {
            return false;
        }
        if control.reason.is_none() {
            control.reason = Some(reason.to_string());
        }
        // Under the lock, so that `commit_swap` sees it (or wins before it).
        self.cancel.cancel();
        true
    }

    pub fn cancel_reason(&self) -> Option<String> {
        lock(&self.control).reason.clone()
    }

    /// The point of no return before routing traffic to the new instances:
    /// `false` if the deploy was cancelled (or the server is shutting down).
    pub fn commit_swap(&self) -> bool {
        let mut control = lock(&self.control);
        if self.cancel.is_cancelled() {
            return false;
        }
        control.swapped = true;
        true
    }

    /// Whether traffic was already switched to this deploy.
    pub fn is_swapped(&self) -> bool {
        lock(&self.control).swapped
    }

    /// Wait (bounded) until the deploy has finished.
    pub async fn wait_done(&self, limit: Duration) -> bool {
        let mut done = self.done.clone();
        matches!(tokio::time::timeout(limit, done.wait_for(|d| *d)).await, Ok(Ok(_)))
    }
}

/// A job run in progress.
#[derive(Clone)]
pub(crate) struct RunningJob {
    pub service_id: String,
    pub cancel: CancellationToken,
    pub done: watch::Receiver<bool>,
}

impl RunningJob {
    pub async fn wait_done(&self, limit: Duration) -> bool {
        let mut done = self.done.clone();
        matches!(tokio::time::timeout(limit, done.wait_for(|d| *d)).await, Ok(Ok(_)))
    }
}

impl Inner {
    pub fn new(config: Arc<Config>, store: Store, docker: Docker, builder: Builder, routes: RouteTable) -> Self {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| format!("cannot create the health-check HTTP client: {e}"));
        Inner {
            naming: config.naming(),
            logs: LogHub::new(config.logs_dir()),
            build_slots: Arc::new(Semaphore::new(config.build_concurrency.max(1))),
            config,
            store,
            docker,
            builder,
            routes,
            http,
            queue_lock: AsyncMutex::new(()),
            service_locks: KeyedLocks::default(),
            datastore_locks: KeyedLocks::default(),
            reconcile_lock: AsyncMutex::new(()),
            reconcile_wake: Notify::new(),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
            started: AtomicBool::new(false),
            worker_generation: AtomicU64::new(1),
            rt: StdMutex::new(Runtime::default()),
        }
    }

    /// Run `f` on the bookkeeping state.
    pub fn with_rt<R>(&self, f: impl FnOnce(&mut Runtime) -> R) -> R {
        f(&mut lock(&self.rt))
    }

    /// Spawn a background task the engine waits for at shutdown.
    pub fn spawn<F>(&self, task: F) -> JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tasks.spawn(task)
    }

    /// Ask the reconcile loop for a pass soon.
    pub fn wake_reconciler(&self) {
        self.reconcile_wake.notify_one();
    }

    pub fn is_deploying(&self, service_id: &str) -> bool {
        self.with_rt(|rt| rt.deploying.contains(service_id))
    }

    pub fn is_service_deleting(&self, service_id: &str) -> bool {
        self.with_rt(|rt| rt.deleting_services.contains(service_id))
    }

    /// Claimed deploys of a service.
    pub fn active_deploys_of(&self, service_id: &str) -> Vec<ActiveDeploy> {
        self.with_rt(|rt| rt.deploys.values().filter(|d| d.service_id == service_id).cloned().collect())
    }

    /// Running jobs of a service.
    pub fn running_jobs_of(&self, service_id: &str) -> Vec<RunningJob> {
        self.with_rt(|rt| rt.jobs.values().filter(|j| j.service_id == service_id).cloned().collect())
    }
}

/// Removes a key from one of the runtime sets when dropped.
pub(crate) struct SetGuard {
    inner: Arc<Inner>,
    key: String,
    set: fn(&mut Runtime) -> &mut HashSet<String>,
}

impl SetGuard {
    /// Insert `key`; `None` if it was already present.
    pub fn insert(inner: &Arc<Inner>, key: &str, set: fn(&mut Runtime) -> &mut HashSet<String>) -> Option<Self> {
        let inserted = inner.with_rt(|rt| set(rt).insert(key.to_string()));
        inserted.then(|| SetGuard { inner: inner.clone(), key: key.to_string(), set })
    }
}

impl Drop for SetGuard {
    fn drop(&mut self) {
        let set = self.set;
        let key = std::mem::take(&mut self.key);
        self.inner.with_rt(|rt| set(rt).remove(&key));
    }
}

pub(crate) fn deploying_set(rt: &mut Runtime) -> &mut HashSet<String> {
    &mut rt.deploying
}

pub(crate) fn deleting_services_set(rt: &mut Runtime) -> &mut HashSet<String> {
    &mut rt.deleting_services
}

pub(crate) fn deleting_datastores_set(rt: &mut Runtime) -> &mut HashSet<String> {
    &mut rt.deleting_datastores
}

pub(crate) fn warmups_set(rt: &mut Runtime) -> &mut HashSet<String> {
    &mut rt.warmups
}
