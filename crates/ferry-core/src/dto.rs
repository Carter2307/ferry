//! Request / response bodies of the HTTP API (`/api/v1`). Shared by the API
//! server, the CLI and (as JSON) the dashboard.
//!
//! Conventions: JSON, snake_case fields; `{id}` path segments accept a
//! resource id **or** its name (an exact id match wins); errors are
//! [`ApiErrorBody`]. Request bodies reject unknown fields.

use serde::{Deserialize, Serialize};

use crate::models::{Datastore, DatastoreKind, Deploy, EnvGroup, EnvVar, Runtime, Service, ServiceState, ServiceType};

/// `GET /api/v1/info`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    pub base_domain: String,
    /// e.g. `http://localhost:8080` — where the proxy listens.
    pub proxy_url: String,
    pub tls_enabled: bool,
    pub dashboard_url: Option<String>,
    pub github_webhook_enabled: bool,
    /// Docker server version, if reachable.
    pub docker_version: Option<String>,
}

/// A service as returned by the API (the stored row plus computed fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceView {
    #[serde(flatten)]
    pub service: Service,
    pub state: ServiceState,
    /// Public URL (web services and static sites).
    pub url: Option<String>,
    /// All hostnames routed to the service.
    pub hosts: Vec<String>,
    /// Hostname on the private network (= name).
    pub internal_host: String,
    /// Port of the live deploy (private network), if known.
    pub internal_port: Option<u16>,
    /// Names of linked env groups, in link order.
    pub env_groups: Vec<String>,
    pub latest_deploy: Option<Deploy>,
    /// `POST` here (no auth header needed) to trigger a deploy.
    pub deploy_hook_path: String,
}

/// `POST /api/v1/services`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateService {
    pub name: String,
    #[serde(rename = "type")]
    pub service_type: Option<ServiceType>,
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub image: Option<String>,
    pub runtime: Option<Runtime>,
    pub root_dir: Option<String>,
    pub dockerfile_path: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub publish_dir: Option<String>,
    pub port: Option<u16>,
    pub health_check_path: Option<String>,
    pub schedule: Option<String>,
    pub instances: Option<u32>,
    pub auto_deploy: Option<bool>,
    pub disk_mount_path: Option<String>,
    pub custom_domains: Option<Vec<String>>,
    /// Initial service env vars.
    pub env: Option<Vec<EnvVar>>,
    /// Env groups to link (ids or names).
    pub env_groups: Option<Vec<String>>,
    /// Queue a first deploy (default: true when repo_url or image is set).
    pub deploy: Option<bool>,
}

/// `PATCH /api/v1/services/{id}` — every field optional. For optional string
/// settings, an empty string clears the value. Changing `instances` scales,
/// `suspended` suspends/resumes, `custom_domains` refreshes routes; build
/// settings take effect on the next deploy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateService {
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub image: Option<String>,
    pub runtime: Option<Runtime>,
    pub root_dir: Option<String>,
    pub dockerfile_path: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub publish_dir: Option<String>,
    /// 0 clears (auto).
    pub port: Option<u16>,
    pub health_check_path: Option<String>,
    pub schedule: Option<String>,
    pub instances: Option<u32>,
    pub auto_deploy: Option<bool>,
    pub suspended: Option<bool>,
    pub disk_mount_path: Option<String>,
    pub custom_domains: Option<Vec<String>>,
}

/// `POST /api/v1/services/{id}/deploys`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerDeploy {
    pub commit: Option<String>,
    #[serde(default)]
    pub clear_cache: bool,
}

/// `POST /api/v1/services/{id}/rollback`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackRequest {
    pub deploy_id: String,
}

/// `POST /api/v1/services/{id}/scale`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleRequest {
    pub instances: u32,
}

/// `PUT /api/v1/services/{id}/env` (replace all) and env group equivalent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceEnv {
    pub vars: Vec<EnvVar>,
}

/// `PATCH /api/v1/services/{id}/env` (and env groups): upsert `set`, delete `unset`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchEnv {
    #[serde(default)]
    pub set: Vec<EnvVar>,
    #[serde(default)]
    pub unset: Vec<String>,
}

/// `POST /api/v1/services/{id}/domains`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainRequest {
    pub domain: String,
}

/// `POST /api/v1/services/{id}/jobs`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunJobRequest {
    /// Shell command (`sh -c`). Required for non-cron services.
    pub command: Option<String>,
}

/// `POST /api/v1/services/{id}/env-groups`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkEnvGroup {
    /// Env group id or name.
    pub group: String,
}

/// Live container state of one instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub container_id: String,
    pub name: String,
    pub deploy_id: Option<String>,
    /// Docker state: created, running, restarting, exited, dead...
    pub state: String,
    pub host_port: Option<u16>,
    pub started_at: Option<String>,
    pub restart_count: Option<i64>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
}

/// `GET /api/v1/services/{id}/status`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub service_id: String,
    pub state: ServiceState,
    pub desired_instances: u32,
    pub instances: Vec<InstanceStatus>,
}

/// `POST /api/v1/datastores`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDatastore {
    pub name: String,
    pub kind: DatastoreKind,
    /// Postgres major (default "16") / Redis major (default "7").
    pub version: Option<String>,
    /// Postgres database name (default: name with `-` → `_`).
    pub database: Option<String>,
    /// Postgres user (default: same as database).
    pub username: Option<String>,
}

/// A datastore with its connection info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatastoreView {
    #[serde(flatten)]
    pub datastore: Datastore,
    pub internal_host: String,
    pub internal_port: u16,
    /// Connection string on the private network (use from services).
    pub internal_url: String,
    /// Connection string from the Ferry host (published on 127.0.0.1).
    pub external_url: Option<String>,
}

/// `POST /api/v1/env-groups`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEnvGroup {
    pub name: String,
    #[serde(default)]
    pub vars: Vec<EnvVar>,
}

/// An env group with its variables and linked services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvGroupView {
    #[serde(flatten)]
    pub group: EnvGroup,
    pub vars: Vec<EnvVar>,
    /// Names of linked services.
    pub services: Vec<String>,
}

/// `POST /api/v1/blueprints/apply` (JSON body) — or send the raw YAML with
/// `Content-Type: application/yaml` and `?dry_run=true`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyBlueprint {
    /// Contents of `ferry.yaml` / `render.yaml`.
    pub yaml: String,
    #[serde(default)]
    pub dry_run: bool,
}

/// One planned/performed change of a blueprint apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintAction {
    /// `service` | `datastore` | `env_group`
    pub resource: String,
    pub name: String,
    /// `create` | `update` | `unchanged`
    pub action: String,
    /// Human-readable list of changed fields.
    #[serde(default)]
    pub changes: Vec<String>,
}

/// Result of a blueprint apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintResult {
    pub dry_run: bool,
    pub actions: Vec<BlueprintAction>,
    /// Deploys queued by the apply.
    pub deploys: Vec<Deploy>,
    /// Non-fatal problems (unsupported keys ignored, etc.).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Error body: `{"error": {"code": "not_found", "message": "service 'x' not found"}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

impl From<&crate::Error> for ApiErrorBody {
    fn from(e: &crate::Error) -> Self {
        ApiErrorBody { error: ApiErrorDetail { code: e.code().to_string(), message: e.to_string() } }
    }
}
