//! Domain models persisted in the store and returned by the API.

use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};

use crate::ids;

/// The current time at the precision the store keeps (microseconds), so a
/// new model equals itself read back from the database (Linux clocks have
/// nanoseconds, macOS microseconds).
pub fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// Declares a string-backed enum with a canonical wire name plus accepted
/// aliases. Serializes to the canonical name; deserializes (and `FromStr`s)
/// from the canonical name or any alias, case-insensitively.
macro_rules! str_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $canon:literal $(| $alias:literal)* ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// Canonical wire name.
            pub fn as_str(&self) -> &'static str {
                match self { $($name::$variant => $canon),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = crate::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let lower = s.trim().to_ascii_lowercase();
                match lower.as_str() {
                    $( $canon $(| $alias)* => Ok($name::$variant), )+
                    _ => Err(crate::Error::invalid(format!(
                        "invalid {} '{}' (expected one of: {})",
                        stringify!($name),
                        s,
                        [$($canon),+].join(", ")
                    ))),
                }
            }
        }

        impl utoipa::PartialSchema for $name {
            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                utoipa::openapi::schema::ObjectBuilder::new()
                    .schema_type(utoipa::openapi::schema::Type::String)
                    .enum_values(Some([$($canon),+]))
                    .into()
            }
        }

        impl utoipa::ToSchema for $name {}

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

str_enum! {
    /// The kind of workload. Mirrors Render's service types.
    pub enum ServiceType {
        /// Public HTTP service, routed by the proxy at `<name>.<base_domain>` + custom domains.
        WebService = "web_service" | "web",
        /// HTTP/TCP service reachable only on the private network at `<name>:<port>`.
        PrivateService = "private_service" | "pserv" | "private",
        /// Long-running process with no port (queue consumers, bots...).
        BackgroundWorker = "background_worker" | "worker",
        /// Runs a command on a cron schedule, then exits.
        CronJob = "cron_job" | "cron",
        /// Build step producing static files, served by nginx behind the proxy.
        StaticSite = "static_site" | "static",
    }
}

str_enum! {
    /// How the image is produced.
    pub enum Runtime {
        /// Detect from the source tree (Dockerfile → docker, package.json → node, ...).
        Auto = "auto",
        /// Build with the repository's Dockerfile.
        Docker = "docker",
        /// Pull a prebuilt image (`Service::image`), no build.
        Image = "image",
        Node = "node" | "nodejs" | "javascript" | "bun",
        Python = "python" | "python3",
        Go = "go" | "golang",
        Rust = "rust",
        Ruby = "ruby",
        /// Serve files as-is (optionally after a node build) with nginx.
        Static = "static",
    }
}

str_enum! {
    /// Lifecycle of a deploy. Mirrors Render's deploy statuses.
    pub enum DeployStatus {
        Queued = "queued" | "created",
        Building = "building" | "build_in_progress",
        Deploying = "deploying" | "update_in_progress",
        /// Currently serving (at most one per service).
        Live = "live",
        /// Was live, replaced by a newer deploy.
        Deactivated = "deactivated",
        BuildFailed = "build_failed",
        DeployFailed = "deploy_failed" | "update_failed",
        Canceled = "canceled" | "cancelled",
    }
}

impl DeployStatus {
    /// Queued, building or deploying.
    pub fn is_active(&self) -> bool {
        matches!(self, DeployStatus::Queued | DeployStatus::Building | DeployStatus::Deploying)
    }

    /// Will never change again (except Live → Deactivated).
    pub fn is_terminal(&self) -> bool {
        !self.is_active()
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, DeployStatus::BuildFailed | DeployStatus::DeployFailed)
    }
}

str_enum! {
    /// What started a deploy.
    pub enum DeployTrigger {
        /// First deploy right after service creation.
        Create = "create",
        /// Explicit request from CLI / dashboard / API.
        Manual = "manual" | "api",
        /// Git push webhook (GitHub).
        Webhook = "webhook" | "git_push",
        /// Secret deploy-hook URL.
        DeployHook = "deploy_hook",
        /// Blueprint (ferry.yaml / render.yaml) apply.
        Blueprint = "blueprint",
        /// Rollback to a previous deploy's image.
        Rollback = "rollback",
        /// Restart: redeploy the live image with current settings/env.
        Restart = "restart",
        /// Environment variables changed.
        EnvChange = "env_change",
        /// Source uploaded from a local directory (`ferry up`).
        Upload = "upload",
    }
}

str_enum! {
    pub enum JobStatus {
        Pending = "pending",
        Running = "running",
        Succeeded = "succeeded",
        Failed = "failed",
        Canceled = "canceled" | "cancelled",
    }
}

impl JobStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobStatus::Succeeded | JobStatus::Failed | JobStatus::Canceled)
    }
}

str_enum! {
    pub enum JobTrigger {
        /// Fired by the cron schedule.
        Schedule = "schedule",
        /// Started by a user (`ferry run`).
        Manual = "manual",
    }
}

str_enum! {
    /// Managed datastores. `redis` is Render's "Key Value".
    pub enum DatastoreKind {
        Postgres = "postgres" | "postgresql" | "pg",
        Redis = "redis" | "keyvalue" | "key_value" | "valkey",
    }
}

str_enum! {
    pub enum DatastoreStatus {
        Creating = "creating",
        Available = "available",
        Failed = "failed",
    }
}

str_enum! {
    /// Computed, user-facing state of a service.
    pub enum ServiceState {
        /// No deploy has ever succeeded and none is in progress.
        NotDeployed = "not_deployed",
        /// A deploy is queued/building/deploying.
        Deploying = "deploying",
        /// Latest deploy is live (for cron jobs: image ready, schedule active).
        Live = "live",
        /// Latest deploy failed and nothing is live.
        Failed = "failed",
        /// Suspended by the user: no containers, proxy answers 503.
        Suspended = "suspended",
        /// Live, but fewer instances are running than desired.
        Degraded = "degraded",
    }
}

/// Where a service's code comes from (derived from its fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// `repo_url` is set: clone and build.
    Git,
    /// `image` is set: pull a prebuilt image.
    Image,
    /// Neither: code arrives via `ferry up` uploads.
    Upload,
}

/// A deployable workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Service {
    pub id: String,
    /// Unique DNS-label name; also the private-network hostname.
    pub name: String,
    #[serde(rename = "type")]
    pub service_type: ServiceType,
    /// Git URL (https, ssh, or local path / file://). Mutually exclusive with `image`.
    pub repo_url: Option<String>,
    /// Branch to build and to watch for auto-deploys.
    pub branch: String,
    /// Prebuilt image reference (`runtime` = image).
    pub image: Option<String>,
    pub runtime: Runtime,
    /// Subdirectory of the repo used as build context (monorepos).
    pub root_dir: Option<String>,
    /// Dockerfile path relative to `root_dir` (default `Dockerfile`).
    pub dockerfile_path: Option<String>,
    /// Build command for native runtimes (e.g. `npm run build`).
    pub build_command: Option<String>,
    /// Start command; overrides the image CMD (run via `sh -c`).
    pub start_command: Option<String>,
    /// Directory of built files to serve (static sites).
    pub publish_dir: Option<String>,
    /// Container port the app listens on. `None` = auto (see `env::choose_port`).
    pub port: Option<u16>,
    /// HTTP path that must answer 2xx/3xx before a new deploy receives traffic.
    pub health_check_path: Option<String>,
    /// 5-field cron expression (cron jobs only), evaluated in UTC.
    pub schedule: Option<String>,
    /// Desired number of containers (web/private/worker).
    pub instances: u32,
    /// Redeploy automatically on git push to `branch`.
    pub auto_deploy: bool,
    pub suspended: bool,
    /// Persistent volume mount path. A service with a disk uses recreate
    /// (stop-then-start) deploys and is limited to one instance.
    pub disk_mount_path: Option<String>,
    /// Memory limit of each instance (and of the service's jobs), in MiB.
    /// `None` = the server default (`ferryd --default-memory-limit`). Takes
    /// effect with the next deploy or restart.
    #[serde(default)]
    pub memory_limit_mb: Option<u32>,
    /// CPU limit of each instance (and of the service's jobs), in CPUs
    /// (`0.5` = half a core). `None` = the server default
    /// (`ferryd --default-cpu-limit`). Takes effect with the next deploy or
    /// restart.
    #[serde(default)]
    pub cpu_limit: Option<f64>,
    /// Extra hostnames routed to this service (web services and static sites).
    pub custom_domains: Vec<String>,
    /// Secret for `POST /hooks/deploy/{service_id}?key=...`.
    pub deploy_hook_key: String,
    /// The deploy currently live, if any.
    pub live_deploy_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Service {
    /// A new service with defaults: branch `main`, runtime auto, 1 instance, auto-deploy on.
    pub fn new(name: impl Into<String>, service_type: ServiceType) -> Self {
        let now = now();
        Service {
            id: ids::new_id(ids::SERVICE),
            name: name.into(),
            service_type,
            repo_url: None,
            branch: "main".to_string(),
            image: None,
            runtime: Runtime::Auto,
            root_dir: None,
            dockerfile_path: None,
            build_command: None,
            start_command: None,
            publish_dir: None,
            port: None,
            health_check_path: None,
            schedule: None,
            instances: 1,
            auto_deploy: true,
            suspended: false,
            disk_mount_path: None,
            memory_limit_mb: None,
            cpu_limit: None,
            custom_domains: Vec::new(),
            deploy_hook_key: ids::random_secret(32),
            live_deploy_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Publicly routed through the proxy (web services and static sites).
    pub fn is_public_http(&self) -> bool {
        matches!(self.service_type, ServiceType::WebService | ServiceType::StaticSite)
    }

    /// Listens on a port (and is therefore health-checked).
    pub fn listens(&self) -> bool {
        matches!(self.service_type, ServiceType::WebService | ServiceType::PrivateService | ServiceType::StaticSite)
    }

    /// Runs long-lived containers (everything except cron jobs).
    pub fn is_long_running(&self) -> bool {
        self.service_type != ServiceType::CronJob
    }

    pub fn source_kind(&self) -> SourceKind {
        if self.image.is_some() {
            SourceKind::Image
        } else if self.repo_url.is_some() {
            SourceKind::Git
        } else {
            SourceKind::Upload
        }
    }

    /// Instance count the runtime should maintain (0 when suspended or a cron job).
    pub fn desired_instances(&self) -> u32 {
        if self.suspended || !self.is_long_running() {
            0
        } else if self.disk_mount_path.is_some() {
            self.instances.min(1)
        } else {
            self.instances
        }
    }
}

/// Where a deploy's code / image comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeploySource {
    /// Clone `repo_url` at `branch` (or the exact `commit`) and build.
    Git { repo_url: String, branch: String, commit: Option<String> },
    /// Build from an uploaded `.tar.gz` stored at `path` on the server.
    Archive { path: String },
    /// Pull and run a prebuilt image.
    Image { image: String },
    /// Reuse an already-built local image (rollback / restart / env change).
    Reuse { image: String, from_deploy: Option<String> },
}

/// One attempt to ship a version of a service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Deploy {
    pub id: String,
    pub service_id: String,
    pub status: DeployStatus,
    pub trigger: DeployTrigger,
    pub source: DeploySource,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    /// Image that was built / pulled / reused. Set once the build succeeds.
    pub image: Option<String>,
    /// Container port this deploy's instances listen on (chosen at deploy time
    /// via `env::choose_port`; None for workers / cron jobs).
    pub port: Option<u16>,
    /// Human-readable failure reason.
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Deploy {
    pub fn new(service_id: &str, trigger: DeployTrigger, source: DeploySource) -> Self {
        Deploy {
            id: ids::new_id(ids::DEPLOY),
            service_id: service_id.to_string(),
            status: DeployStatus::Queued,
            trigger,
            source,
            commit_sha: None,
            commit_message: None,
            image: None,
            port: None,
            error: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        }
    }
}

/// A cron-job run or one-off job (`ferry run`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobRun {
    pub id: String,
    pub service_id: String,
    pub trigger: JobTrigger,
    /// Command override (`sh -c`), else the service start command / image CMD.
    pub command: Option<String>,
    pub image: Option<String>,
    pub status: JobStatus,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl JobRun {
    pub fn new(service_id: &str, trigger: JobTrigger, command: Option<String>) -> Self {
        JobRun {
            id: ids::new_id(ids::JOB),
            service_id: service_id.to_string(),
            trigger,
            command,
            image: None,
            status: JobStatus::Pending,
            exit_code: None,
            error: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        }
    }
}

/// A managed Postgres or Redis instance (one container + one volume).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Datastore {
    pub id: String,
    /// Unique DNS-label name; also the private-network hostname.
    pub name: String,
    pub kind: DatastoreKind,
    /// Major version / image tag, e.g. "16" (postgres) or "7" (redis).
    pub version: String,
    pub status: DatastoreStatus,
    /// Postgres user (redis: always "default").
    pub username: String,
    pub password: String,
    /// Postgres database name (redis: None).
    pub database: Option<String>,
    /// Port published on the host for external access (bound to 127.0.0.1).
    pub host_port: Option<u16>,
    /// Memory limit of the container, in MiB. `None` = the server default
    /// (`ferryd --default-memory-limit`).
    #[serde(default)]
    pub memory_limit_mb: Option<u32>,
    /// CPU limit of the container, in CPUs. `None` = the server default
    /// (`ferryd --default-cpu-limit`).
    #[serde(default)]
    pub cpu_limit: Option<f64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Datastore {
    /// A new datastore with generated credentials and default version.
    pub fn new(name: impl Into<String>, kind: DatastoreKind) -> Self {
        let name = name.into();
        let now = now();
        let (version, username, database) = match kind {
            DatastoreKind::Postgres => ("16".to_string(), name.replace('-', "_"), Some(name.replace('-', "_"))),
            DatastoreKind::Redis => ("7".to_string(), "default".to_string(), None),
        };
        Datastore {
            id: ids::new_id(ids::DATASTORE),
            name,
            kind,
            version,
            status: DatastoreStatus::Creating,
            username,
            password: ids::random_secret(32),
            database,
            host_port: None,
            memory_limit_mb: None,
            cpu_limit: None,
            error: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Hostname on the private network (same as the name).
    pub fn internal_host(&self) -> &str {
        &self.name
    }

    /// Port inside the private network.
    pub fn internal_port(&self) -> u16 {
        match self.kind {
            DatastoreKind::Postgres => 5432,
            DatastoreKind::Redis => 6379,
        }
    }

    /// Docker image to run, e.g. `postgres:16-alpine`.
    pub fn image(&self) -> String {
        match self.kind {
            DatastoreKind::Postgres => format!("postgres:{}-alpine", self.version),
            DatastoreKind::Redis => format!("redis:{}-alpine", self.version),
        }
    }

    fn url_for(&self, host: &str, port: u16) -> String {
        match self.kind {
            DatastoreKind::Postgres => format!(
                "postgresql://{}:{}@{}:{}/{}",
                self.username,
                self.password,
                host,
                port,
                self.database.as_deref().unwrap_or("postgres")
            ),
            DatastoreKind::Redis => format!("redis://{}:{}@{}:{}", self.username, self.password, host, port),
        }
    }

    /// Connection string for services on the private network.
    pub fn internal_url(&self) -> String {
        self.url_for(self.internal_host(), self.internal_port())
    }

    /// Connection string from the host machine (None until a host port is assigned).
    pub fn external_url(&self, host: &str) -> Option<String> {
        self.host_port.map(|p| self.url_for(host, p))
    }
}

/// A plain environment variable. Values may contain references such as
/// `${{datastore.db.connectionString}}` (see [`crate::env`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

impl EnvVar {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        EnvVar { key: key.into(), value: value.into() }
    }
}

/// A named, reusable set of environment variables linked to services.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EnvGroup {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl EnvGroup {
    pub fn new(name: impl Into<String>) -> Self {
        let now = now();
        EnvGroup { id: ids::new_id(ids::ENV_GROUP), name: name.into(), created_at: now, updated_at: now }
    }
}

/// Compute the user-facing state from the stored service and its deploys.
/// `running_instances` is `None` when runtime information is unavailable.
pub fn compute_service_state(
    service: &Service,
    latest_deploy: Option<&Deploy>,
    running_instances: Option<u32>,
) -> ServiceState {
    if service.suspended {
        return ServiceState::Suspended;
    }
    if latest_deploy.is_some_and(|d| d.status.is_active()) {
        return ServiceState::Deploying;
    }
    if service.live_deploy_id.is_some() {
        if let Some(running) = running_instances
            && running < service.desired_instances()
        {
            return ServiceState::Degraded;
        }
        return ServiceState::Live;
    }
    match latest_deploy {
        Some(d) if d.status.is_failed() => ServiceState::Failed,
        _ => ServiceState::NotDeployed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_accept_aliases_and_serialize_canonically() {
        assert_eq!("web".parse::<ServiceType>().unwrap(), ServiceType::WebService);
        assert_eq!("PSERV".parse::<ServiceType>().unwrap(), ServiceType::PrivateService);
        assert_eq!("keyvalue".parse::<DatastoreKind>().unwrap(), DatastoreKind::Redis);
        assert!("nope".parse::<Runtime>().is_err());
        let json = serde_json::to_string(&ServiceType::CronJob).unwrap();
        assert_eq!(json, "\"cron_job\"");
        let back: ServiceType = serde_json::from_str("\"cron\"").unwrap();
        assert_eq!(back, ServiceType::CronJob);
    }

    #[test]
    fn datastore_urls() {
        let mut ds = Datastore::new("main-db", DatastoreKind::Postgres);
        ds.password = "pw".into();
        assert_eq!(ds.internal_url(), "postgresql://main_db:pw@main-db:5432/main_db");
        assert_eq!(ds.external_url("127.0.0.1"), None);
        ds.host_port = Some(15432);
        assert_eq!(ds.external_url("127.0.0.1").unwrap(), "postgresql://main_db:pw@127.0.0.1:15432/main_db");
        let mut r = Datastore::new("cache", DatastoreKind::Redis);
        r.password = "pw".into();
        assert_eq!(r.internal_url(), "redis://default:pw@cache:6379");
    }

    #[test]
    fn deploy_source_serde() {
        let s = DeploySource::Git { repo_url: "u".into(), branch: "main".into(), commit: None };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["kind"], "git");
        let back: DeploySource = serde_json::from_value(j).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn state_computation() {
        let mut svc = Service::new("web", ServiceType::WebService);
        assert_eq!(compute_service_state(&svc, None, None), ServiceState::NotDeployed);
        let mut d = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        assert_eq!(compute_service_state(&svc, Some(&d), None), ServiceState::Deploying);
        d.status = DeployStatus::BuildFailed;
        assert_eq!(compute_service_state(&svc, Some(&d), None), ServiceState::Failed);
        svc.live_deploy_id = Some("dep-x".into());
        assert_eq!(compute_service_state(&svc, Some(&d), Some(1)), ServiceState::Live);
        svc.instances = 2;
        assert_eq!(compute_service_state(&svc, Some(&d), Some(1)), ServiceState::Degraded);
        svc.suspended = true;
        assert_eq!(compute_service_state(&svc, Some(&d), Some(0)), ServiceState::Suspended);
    }
}
