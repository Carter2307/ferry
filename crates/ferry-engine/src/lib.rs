//! # ferry-engine
//!
//! The brain of Ferry. Implements [`ferry_core::Engine`]:
//! * deploy queue + workers (build → start → health check → route swap →
//!   drain old), rollbacks, restarts, cancellation;
//! * a reconciler converging Docker to the desired state in the store
//!   (instance counts, routes, suspended services, crash recovery);
//! * the cron scheduler and one-off jobs;
//! * managed datastores (Postgres / Redis containers + volumes);
//! * deploy/job log storage and live following.

use std::sync::Arc;

use async_trait::async_trait;
use ferry_build::Builder;
use ferry_core::dto::RuntimeStatus;
use ferry_core::{
    CancellationToken, Config, Deploy, DeployRequest, DeployTrigger, Engine, JobRun, JobTrigger, LogOptions, LogStream,
    Result, Store,
};
use ferry_docker::Docker;
use ferry_proxy::RouteTable;

/// The Ferry engine. Create with [`FerryEngine::new`], then call
/// [`FerryEngine::start`] once.
pub struct FerryEngine {
    config: Arc<Config>,
    store: Store,
    docker: Docker,
    builder: Builder,
    routes: RouteTable,
}

impl FerryEngine {
    pub fn new(config: Arc<Config>, store: Store, docker: Docker, builder: Builder, routes: RouteTable) -> Arc<Self> {
        Arc::new(FerryEngine { config, store, docker, builder, routes })
    }

    /// Boot sequence, then return (background tasks keep running until
    /// `shutdown` is cancelled):
    /// 1. ensure the Docker network and data directories exist;
    /// 2. mark deploys/jobs interrupted by a previous crash as failed;
    /// 3. reconcile once synchronously (so routes are live before the proxy
    ///    starts serving);
    /// 4. spawn deploy workers, the reconcile loop, the cron scheduler and
    ///    datastore provisioning for rows still `creating`.
    pub async fn start(self: &Arc<Self>, shutdown: CancellationToken) -> Result<()> {
        let _ = (shutdown, &self.config, &self.store, &self.docker, &self.builder, &self.routes);
        todo!("ferry-engine: start")
    }
}

#[async_trait]
impl Engine for FerryEngine {
    async fn deploy(&self, service_id: &str, req: DeployRequest) -> Result<Deploy> {
        let _ = (service_id, req);
        todo!("ferry-engine: deploy")
    }

    async fn cancel_deploy(&self, deploy_id: &str) -> Result<Deploy> {
        let _ = deploy_id;
        todo!("ferry-engine: cancel_deploy")
    }

    async fn rollback(&self, service_id: &str, deploy_id: &str) -> Result<Deploy> {
        let _ = (service_id, deploy_id);
        todo!("ferry-engine: rollback")
    }

    async fn restart(&self, service_id: &str, trigger: DeployTrigger) -> Result<Deploy> {
        let _ = (service_id, trigger);
        todo!("ferry-engine: restart")
    }

    async fn suspend(&self, service_id: &str) -> Result<()> {
        let _ = service_id;
        todo!("ferry-engine: suspend")
    }

    async fn resume(&self, service_id: &str) -> Result<()> {
        let _ = service_id;
        todo!("ferry-engine: resume")
    }

    async fn scale(&self, service_id: &str, instances: u32) -> Result<()> {
        let _ = (service_id, instances);
        todo!("ferry-engine: scale")
    }

    async fn delete_service(&self, service_id: &str) -> Result<()> {
        let _ = service_id;
        todo!("ferry-engine: delete_service")
    }

    async fn refresh_routes(&self, service_id: &str) -> Result<()> {
        let _ = service_id;
        todo!("ferry-engine: refresh_routes")
    }

    async fn service_status(&self, service_id: &str) -> Result<RuntimeStatus> {
        let _ = service_id;
        todo!("ferry-engine: service_status")
    }

    async fn service_logs(&self, service_id: &str, opts: LogOptions) -> Result<LogStream> {
        let _ = (service_id, opts);
        todo!("ferry-engine: service_logs")
    }

    async fn deploy_logs(&self, deploy_id: &str, follow: bool) -> Result<LogStream> {
        let _ = (deploy_id, follow);
        todo!("ferry-engine: deploy_logs")
    }

    async fn run_job(&self, service_id: &str, command: Option<String>, trigger: JobTrigger) -> Result<JobRun> {
        let _ = (service_id, command, trigger);
        todo!("ferry-engine: run_job")
    }

    async fn job_logs(&self, job_id: &str, follow: bool) -> Result<LogStream> {
        let _ = (job_id, follow);
        todo!("ferry-engine: job_logs")
    }

    async fn provision_datastore(&self, datastore_id: &str) -> Result<()> {
        let _ = datastore_id;
        todo!("ferry-engine: provision_datastore")
    }

    async fn delete_datastore(&self, datastore_id: &str) -> Result<()> {
        let _ = datastore_id;
        todo!("ferry-engine: delete_datastore")
    }
}
