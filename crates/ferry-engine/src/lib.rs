//! # ferry-engine
//!
//! The brain of Ferry. Implements [`ferry_core::Engine`]:
//! * deploy queue + workers (build → start → health check → route swap →
//!   drain old), rollbacks, restarts, cancellation;
//! * a reconciler converging Docker to the desired state in the store
//!   (instance counts, routes, suspended services, crash recovery);
//! * the cron scheduler and one-off jobs (cancel, retention);
//! * managed datastores (Postgres / Redis containers + volumes);
//! * deploy/job log storage and live following.
//!
//! Module map: `deploy` (queue, workers, cancel), `pipeline` (one deploy),
//! `spec` (the launch spec a live deploy's instances and jobs run with),
//! `instances` (container specs, ports, disks), `health` (probes),
//! `reconcile` (convergence, routes, route watcher), `ops`
//! (suspend/resume/scale/delete), `jobs` (one-off jobs + cron),
//! `datastores`, `status` (status + runtime logs), `logs` (deploy/job log
//! hub), `images` (pull policy + retention), `limits` (container resource
//! limits, OOM messages, free-disk check), `domains` (the domains services
//! are served under, and their verification).

mod datastores;
mod deploy;
mod domains;
mod health;
mod images;
mod instances;
mod jobs;
mod limits;
mod logs;
mod ops;
mod pipeline;
mod reconcile;
mod spec;
mod state;
mod status;
#[cfg(test)]
mod tests;
mod util;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use ferry_build::Builder;
use ferry_core::dto::RuntimeStatus;
use ferry_core::{
    CancellationToken, Config, Deploy, DeployRequest, DeployTrigger, Domain, Engine, Error, JobRun, JobTrigger,
    LogOptions, LogStream, Result, Store,
};
use ferry_docker::Docker;
use ferry_proxy::RouteTable;
use tracing::info;

use crate::logs::LogKind;
use crate::state::Inner;

/// The Ferry engine. Create with [`FerryEngine::new`], then call
/// [`FerryEngine::start`] once.
pub struct FerryEngine {
    inner: Arc<Inner>,
}

impl FerryEngine {
    pub fn new(config: Arc<Config>, store: Store, docker: Docker, builder: Builder, routes: RouteTable) -> Arc<Self> {
        Arc::new(FerryEngine { inner: Arc::new(Inner::new(config, store, docker, builder, routes)) })
    }

    /// Boot sequence, then return (background tasks keep running until
    /// `shutdown` is cancelled; [`FerryEngine::stopped`] waits for them):
    /// 1. ensure the Docker network and data directories exist;
    /// 2. mark deploys/jobs interrupted by a previous crash as failed;
    /// 3. read the domains services are served under (the base domain and
    ///    the connected ones, DESIGN.md §21);
    /// 4. install the routes of every service synchronously (so they are live
    ///    before the proxy starts serving); the rest of the boot convergence
    ///    (stopping stale instances, starting missing ones) continues in the
    ///    background;
    /// 5. spawn deploy workers, the reconcile loop, the route and OOM
    ///    watchers, the cron scheduler, the domain verifier and datastore
    ///    provisioning for rows still `creating`.
    ///
    /// Calling it twice is a `Conflict` error.
    pub async fn start(self: &Arc<Self>, shutdown: CancellationToken) -> Result<()> {
        let inner = &self.inner;
        if inner.started.swap(true, Ordering::SeqCst) {
            return Err(Error::conflict("the engine is already started"));
        }
        // Everything the engine spawns stops with `shutdown`.
        {
            let internal = inner.shutdown.clone();
            inner.spawn(async move {
                shutdown.cancelled().await;
                internal.cancel();
            });
        }

        let config = &inner.config;
        for dir in [
            inner.logs.dir(LogKind::Deploy),
            inner.logs.dir(LogKind::Job),
            config.builds_dir(),
            config.repos_dir(),
            config.uploads_dir(),
        ] {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| Error::internal(format!("creating directory {}: {e}", dir.display())))?;
        }
        inner.docker.ensure_network(&inner.naming.network(), &inner.naming.base_labels("network")).await?;

        // No build runs yet: scratch directories are leftovers of a crash.
        remove_stale_build_dirs(&config.builds_dir()).await;
        deploy::recover_interrupted(inner).await?;
        // Before any route: a service's hostnames come from the domains.
        ferry_core::domains::init(&inner.store, config).await?;
        reconcile::reconcile_all(inner, reconcile::Pass::Boot).await?;

        inner.spawn(reconcile::run_loop(inner.clone()));
        inner.spawn(reconcile::watch_routes(inner.clone()));
        inner.spawn(reconcile::watch_oom(inner.clone()));
        inner.spawn(jobs::run_scheduler(inner.clone()));
        inner.spawn(domains::run_verifier(inner.clone()));
        inner.spawn(deploy::fail_queued_on_shutdown(inner.clone()));
        datastores::resume_provisioning(inner).await?;
        info!(prefix = %inner.naming.prefix(), "engine started");
        Ok(())
    }

    /// Resolves once the `shutdown` token given to [`FerryEngine::start`] is
    /// cancelled and every background task has wound down: running deploys
    /// recorded as interrupted (with their new instances removed), queued
    /// ones failed with their logs finished, running jobs stopped (with their
    /// grace period) and recorded, loops stopped. Await it (with a timeout)
    /// before exiting the process. Resolves at once if the engine was never
    /// started.
    pub async fn stopped(&self) {
        let inner = &self.inner;
        if !inner.started.load(Ordering::SeqCst) {
            return;
        }
        inner.shutdown.cancelled().await;
        inner.tasks.close();
        inner.tasks.wait().await;
    }
}

/// Delete what a previous process left in `builds/` (best effort).
async fn remove_stale_build_dirs(dir: &std::path::Path) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!(dir = %dir.display(), "cannot list build directories: {e}");
            return;
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let removed = match entry.file_type().await {
            Ok(t) if t.is_dir() => tokio::fs::remove_dir_all(&path).await,
            Ok(_) => tokio::fs::remove_file(&path).await,
            Err(e) => Err(e),
        };
        match removed {
            Ok(()) => info!(path = %path.display(), "removed stale build directory"),
            Err(e) => tracing::warn!(path = %path.display(), "cannot remove stale build directory: {e}"),
        }
    }
}

#[async_trait]
impl Engine for FerryEngine {
    async fn deploy(&self, service_id: &str, req: DeployRequest) -> Result<Deploy> {
        deploy::deploy(&self.inner, service_id, req).await
    }

    async fn cancel_deploy(&self, deploy_id: &str) -> Result<Deploy> {
        deploy::cancel_deploy(&self.inner, deploy_id).await
    }

    async fn rollback(&self, service_id: &str, deploy_id: &str) -> Result<Deploy> {
        deploy::rollback(&self.inner, service_id, deploy_id).await
    }

    async fn restart(&self, service_id: &str, trigger: DeployTrigger) -> Result<Deploy> {
        deploy::restart(&self.inner, service_id, trigger).await
    }

    async fn suspend(&self, service_id: &str) -> Result<()> {
        ops::suspend(&self.inner, service_id).await
    }

    async fn resume(&self, service_id: &str) -> Result<()> {
        ops::resume(&self.inner, service_id).await
    }

    async fn scale(&self, service_id: &str, instances: u32) -> Result<()> {
        ops::scale(&self.inner, service_id, instances).await
    }

    async fn delete_service(&self, service_id: &str) -> Result<()> {
        ops::delete_service(&self.inner, service_id).await
    }

    async fn refresh_routes(&self, service_id: &str) -> Result<()> {
        ops::refresh_routes(&self.inner, service_id).await
    }

    async fn service_status(&self, service_id: &str) -> Result<RuntimeStatus> {
        status::service_status(&self.inner, service_id).await
    }

    async fn service_logs(&self, service_id: &str, opts: LogOptions) -> Result<LogStream> {
        status::service_logs(&self.inner, service_id, opts).await
    }

    async fn deploy_logs(&self, deploy_id: &str, follow: bool) -> Result<LogStream> {
        let d = self.inner.store.require_deploy(deploy_id).await?;
        Ok(self.inner.logs.stream(LogKind::Deploy, &d.id, follow))
    }

    async fn run_job(&self, service_id: &str, command: Option<String>, trigger: JobTrigger) -> Result<JobRun> {
        jobs::run_job(&self.inner, service_id, command, trigger).await
    }

    async fn job_logs(&self, job_id: &str, follow: bool) -> Result<LogStream> {
        let j = self.inner.store.require_job_run(job_id).await?;
        Ok(self.inner.logs.stream(LogKind::Job, &j.id, follow))
    }

    async fn cancel_job(&self, job_id: &str) -> Result<JobRun> {
        jobs::cancel_job(&self.inner, job_id).await
    }

    async fn provision_datastore(&self, datastore_id: &str) -> Result<()> {
        datastores::provision(&self.inner, datastore_id).await
    }

    async fn delete_datastore(&self, datastore_id: &str) -> Result<()> {
        datastores::delete(&self.inner, datastore_id).await
    }

    async fn update_datastore_limits(&self, datastore_id: &str) -> Result<()> {
        datastores::update_limits(&self.inner, datastore_id).await
    }

    async fn refresh_domains(&self) -> Result<()> {
        domains::refresh(&self.inner).await
    }

    async fn verify_domain(&self, domain_id: &str) -> Result<Domain> {
        domains::verify(&self.inner, domain_id).await
    }

    async fn public_addresses(&self) -> Vec<std::net::IpAddr> {
        domains::public_addresses(&self.inner).await
    }
}
