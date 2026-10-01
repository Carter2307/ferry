//! The [`Engine`] trait: every side-effecting runtime operation the API needs.
//!
//! `ferry-api` only knows this trait (plus the [`crate::Store`] for plain
//! CRUD); `ferry-engine` implements it. `ferryd` wires the two together.
//!
//! Division of labour:
//! * The API validates input and performs pure data changes through the Store
//!   (create/update service rows, env vars, env groups, datastore rows...).
//! * Anything touching Docker, builds, the proxy or background work goes
//!   through the Engine.

use async_trait::async_trait;

use crate::Result;
use crate::dto::RuntimeStatus;
use crate::logs::LogStream;
use crate::models::{Deploy, DeploySource, DeployTrigger, Domain, JobRun, JobTrigger};

/// Parameters for [`Engine::deploy`].
#[derive(Debug, Clone)]
pub struct DeployRequest {
    pub trigger: DeployTrigger,
    /// Explicit source. `None` = derive from the service: git (repo_url/branch),
    /// image, or — for upload-only services — the most recent uploaded archive.
    pub source: Option<DeploySource>,
    /// Git commit to build (only meaningful for git sources).
    pub commit: Option<String>,
    /// Build without the Docker layer cache.
    pub clear_cache: bool,
}

impl DeployRequest {
    pub fn new(trigger: DeployTrigger) -> Self {
        DeployRequest { trigger, source: None, commit: None, clear_cache: false }
    }
}

/// Options for runtime log streams.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogOptions {
    /// Keep the stream open and forward new lines.
    pub follow: bool,
    /// Only the last N lines per instance (None = all available).
    pub tail: Option<usize>,
}

#[async_trait]
pub trait Engine: Send + Sync + 'static {
    /// Queue a deploy and return it immediately (status `queued`). Deploys of
    /// one service run one at a time; a newer queued deploy supersedes (cancels)
    /// older *queued* ones. Errors: service not found (NotFound), service is
    /// suspended (Conflict), or no source available (Invalid).
    async fn deploy(&self, service_id: &str, req: DeployRequest) -> Result<Deploy>;

    /// Cancel a queued or in-progress deploy. Returns the updated deploy.
    async fn cancel_deploy(&self, deploy_id: &str) -> Result<Deploy>;

    /// Queue a deploy that reuses the image of `deploy_id` (a previous deploy
    /// of the same service whose image still exists).
    async fn rollback(&self, service_id: &str, deploy_id: &str) -> Result<Deploy>;

    /// Queue a deploy that reuses the live image with the current settings and
    /// env (used after env var / env group changes, and for plain restarts).
    async fn restart(&self, service_id: &str, trigger: DeployTrigger) -> Result<Deploy>;

    /// Stop all containers, mark the service suspended, proxy answers 503.
    async fn suspend(&self, service_id: &str) -> Result<()>;

    /// Undo `suspend`: start the live deploy's containers again.
    async fn resume(&self, service_id: &str) -> Result<()>;

    /// Persist the new instance count and converge containers to it
    /// (in place, no new deploy).
    async fn scale(&self, service_id: &str, instances: u32) -> Result<()>;

    /// Tear down containers, routes, images and disk volume, then delete the
    /// service and its deploys/jobs/env from the store.
    async fn delete_service(&self, service_id: &str) -> Result<()>;

    /// Re-read the service's hosts (default + custom domains) and update the
    /// proxy routes / certificate requests. Called after domain changes.
    async fn refresh_routes(&self, service_id: &str) -> Result<()>;

    /// Live container information for a service.
    async fn service_status(&self, service_id: &str) -> Result<RuntimeStatus>;

    /// Runtime logs of the service's current containers (merged, each line
    /// tagged with its `instance`).
    async fn service_logs(&self, service_id: &str, opts: LogOptions) -> Result<LogStream>;

    /// Build + deploy logs of a deploy. With `follow`, the stream stays open
    /// until the deploy reaches a terminal state.
    async fn deploy_logs(&self, deploy_id: &str, follow: bool) -> Result<LogStream>;

    /// Start a job: for cron jobs, a run of the schedule's command now; for any
    /// other service, a one-off container from the live image running
    /// `command` (required for non-cron services). Returns immediately.
    async fn run_job(&self, service_id: &str, command: Option<String>, trigger: JobTrigger) -> Result<JobRun>;

    /// Output of a job run. With `follow`, stays open until the job finishes.
    async fn job_logs(&self, job_id: &str, follow: bool) -> Result<LogStream>;

    /// Stop a pending or running job (the container gets a 10s grace period)
    /// and mark it `canceled`. Conflict when it already finished. Returns the
    /// updated job.
    async fn cancel_job(&self, job_id: &str) -> Result<JobRun> {
        let _ = job_id;
        Err(crate::Error::invalid("canceling jobs is not supported by this engine"))
    }

    /// Start provisioning a datastore whose row already exists in the store
    /// (status `creating`). Returns immediately; status becomes `available`
    /// or `failed` later.
    async fn provision_datastore(&self, datastore_id: &str) -> Result<()>;

    /// Remove the datastore container and volume, then the store row.
    async fn delete_datastore(&self, datastore_id: &str) -> Result<()>;

    /// Apply the datastore's stored resource limits (see
    /// [`crate::Config::limits`]) to its container in place, without a
    /// restart. Called after `PATCH /api/v1/datastores/{id}`. A datastore that
    /// has no container yet picks its limits up when it is provisioned.
    async fn update_datastore_limits(&self, datastore_id: &str) -> Result<()> {
        let _ = datastore_id;
        Err(crate::Error::invalid("changing datastore limits is not supported by this engine"))
    }

    /// The domains in the store changed (one was connected, removed or made
    /// the default): read them again, so every service is served — proxy
    /// routes, certificate requests — under the domains that are served now,
    /// and a domain that waits for its DNS is verified soon (§21).
    async fn refresh_domains(&self) -> Result<()> {
        Ok(())
    }

    /// Verify now whether the names under a domain reach this server, store
    /// what was found and return the updated domain. Called by
    /// `POST /api/v1/domains/{id}/verify`; the engine also does it on its
    /// own, on a schedule.
    async fn verify_domain(&self, domain_id: &str) -> Result<Domain> {
        let _ = domain_id;
        Err(crate::Error::invalid("verifying domains is not supported by this engine"))
    }

    /// The addresses this server is reached at from the internet, for the
    /// DNS records of a domain: `Config::public_ips`, else what the engine
    /// finds out (and remembers). Empty when it can't tell.
    async fn public_addresses(&self) -> Vec<std::net::IpAddr> {
        Vec::new()
    }
}
