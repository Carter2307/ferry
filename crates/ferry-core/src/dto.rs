//! Request / response bodies of the HTTP API (`/api/v1`). Shared by the API
//! server, the CLI and (as JSON) the dashboard.
//!
//! Conventions: JSON, snake_case fields; `{id}` path segments accept a
//! resource id **or** its name (an exact id match wins); errors are
//! [`ApiErrorBody`]. Request bodies reject unknown fields.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use crate::models::{
    Datastore, DatastoreKind, Deploy, EnvGroup, EnvVar, GitAuth, GitProvider, Runtime, Service, ServiceState,
    ServiceType,
};

/// `GET /api/v1/info`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Memory limit (MiB) of containers that set none. 0 = unlimited.
    #[serde(default)]
    pub default_memory_limit_mb: u32,
    /// CPU limit (CPUs) of containers that set none. 0 = unlimited.
    #[serde(default)]
    pub default_cpu_limit: f64,
    /// CPUs of the Docker host, if known.
    #[serde(default)]
    pub docker_cpus: Option<u32>,
    /// Total memory of the Docker host (bytes), if known.
    #[serde(default)]
    pub docker_memory_bytes: Option<u64>,
}

/// A service as returned by the API (the stored row plus computed fields).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Memory limit per instance, in MiB. Omitted or 0 = the server default.
    pub memory_limit_mb: Option<u32>,
    /// CPU limit per instance, in CPUs. Omitted or 0 = the server default.
    pub cpu_limit: Option<f64>,
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
/// settings and resource limits take effect on the next deploy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Memory limit per instance, in MiB. 0 clears (server default). Takes
    /// effect with the next deploy or restart.
    pub memory_limit_mb: Option<u32>,
    /// CPU limit per instance, in CPUs. 0 clears (server default). Takes
    /// effect with the next deploy or restart.
    pub cpu_limit: Option<f64>,
    pub custom_domains: Option<Vec<String>>,
}

/// `POST /api/v1/services/{id}/deploys`
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerDeploy {
    pub commit: Option<String>,
    #[serde(default)]
    pub clear_cache: bool,
}

/// `POST /api/v1/services/{id}/rollback`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RollbackRequest {
    pub deploy_id: String,
}

/// `POST /api/v1/services/{id}/scale`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScaleRequest {
    pub instances: u32,
}

/// `PUT /api/v1/services/{id}/env` (replace all) and env group equivalent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceEnv {
    pub vars: Vec<EnvVar>,
}

/// `PATCH /api/v1/services/{id}/env` (and env groups): upsert `set`, delete `unset`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchEnv {
    #[serde(default)]
    pub set: Vec<EnvVar>,
    #[serde(default)]
    pub unset: Vec<String>,
}

/// `POST /api/v1/services/{id}/domains`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DomainRequest {
    pub domain: String,
}

/// `POST /api/v1/services/{id}/jobs`
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunJobRequest {
    /// Shell command (`sh -c`). Required for non-cron services.
    pub command: Option<String>,
}

/// `POST /api/v1/services/{id}/env-groups`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkEnvGroup {
    /// Env group id or name.
    pub group: String,
}

/// Live container state of one instance.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// The container's memory limit (bytes); `None` when unlimited.
    pub memory_limit_bytes: Option<u64>,
    /// The container's CPU limit (CPUs); `None` when unlimited.
    #[serde(default)]
    pub cpu_limit: Option<f64>,
    /// The kernel killed the container's process for exceeding its memory
    /// limit (the last time it exited).
    #[serde(default)]
    pub oom_killed: bool,
    /// Exit code of the last exit, for exited / restarting containers.
    #[serde(default)]
    pub exit_code: Option<i64>,
}

/// `GET /api/v1/services/{id}/status`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RuntimeStatus {
    pub service_id: String,
    pub state: ServiceState,
    pub desired_instances: u32,
    pub instances: Vec<InstanceStatus>,
}

/// `POST /api/v1/datastores`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Memory limit, in MiB. Omitted or 0 = the server default.
    pub memory_limit_mb: Option<u32>,
    /// CPU limit, in CPUs. Omitted or 0 = the server default.
    pub cpu_limit: Option<f64>,
}

/// `PATCH /api/v1/datastores/{id}` — change resource limits. 0 clears
/// (server default). Applied to the running container in place (no restart).
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDatastore {
    /// Memory limit, in MiB. 0 clears (server default); omitted keeps it.
    pub memory_limit_mb: Option<u32>,
    /// CPU limit, in CPUs. 0 clears (server default); omitted keeps it.
    pub cpu_limit: Option<f64>,
}

/// A datastore with its connection info.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateEnvGroup {
    pub name: String,
    #[serde(default)]
    pub vars: Vec<EnvVar>,
}

/// An env group with its variables and linked services.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EnvGroupView {
    #[serde(flatten)]
    pub group: EnvGroup,
    pub vars: Vec<EnvVar>,
    /// Names of linked services.
    pub services: Vec<String>,
}

/// `POST /api/v1/git/authorize` — start, or resume, the authorization of a
/// git provider account in the browser.
#[derive(Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorizeGit {
    /// The provider to authorize. Not needed with `connection_id`.
    pub provider: Option<GitProvider>,
    /// Web URL of a self-hosted instance (GitHub Enterprise Server, GitLab
    /// self-managed), e.g. `https://gitlab.example.com`. Default:
    /// `https://github.com` / `https://gitlab.com`.
    pub base_url: Option<String>,
    /// Resume the authorization of this (pending) connection, or authorize
    /// it again.
    pub connection_id: Option<String>,
    /// Where the provider sends the browser back when it is done: the
    /// dashboard's `/git/callback` page, which hands the query parameters
    /// it receives to `POST /api/v1/git/callback`.
    pub redirect_uri: String,
    /// GitHub: register the app in this organization instead of the
    /// user's own account, to reach the organization's repositories.
    pub organization: Option<String>,
    /// GitLab: the Application ID of the OAuth application created for
    /// this server. Needed the first time (and to replace the application).
    pub client_id: Option<String>,
    /// GitLab: the application's secret.
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for AuthorizeGit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizeGit")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("connection_id", &self.connection_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("organization", &self.organization)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "***"))
            .finish()
    }
}

/// `POST /api/v1/git/callback` — the query parameters the provider sent the
/// browser back with.
#[derive(Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCallback {
    /// The `state` parameter: names the authorization this answers.
    pub state: String,
    /// The `code` parameter.
    pub code: Option<String>,
    /// GitHub: the `installation_id` parameter.
    pub installation_id: Option<u64>,
    /// GitHub: the `setup_action` parameter (`install`, `update`, `request`).
    pub setup_action: Option<String>,
    /// The `error` parameter, when the provider reports one (the user
    /// refused, the application is misconfigured).
    pub error: Option<String>,
    pub error_description: Option<String>,
}

impl std::fmt::Debug for GitCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitCallback")
            .field("code", &self.code.as_ref().map(|_| "***"))
            .field("installation_id", &self.installation_id)
            .field("setup_action", &self.setup_action)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// What a browser authorization needs next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitAuthorizationStatus {
    /// Send the browser to `url`.
    Redirect,
    /// Nothing: the account is connected.
    Connected,
}

/// How the browser is sent to the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitRedirectMethod {
    /// Navigate to the URL.
    Get,
    /// Submit a form with `fields` to the URL.
    Post,
}

/// The next step of a browser authorization (`POST /api/v1/git/authorize`
/// and `POST /api/v1/git/callback`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GitAuthorization {
    pub status: GitAuthorizationStatus,
    /// `redirect`: the provider's page to send the browser to.
    pub url: Option<String>,
    /// `redirect`: how.
    pub method: Option<GitRedirectMethod>,
    /// `redirect` with `post`: the fields of the form to submit.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// `connected`: the connection.
    pub connection: Option<GitConnectionView>,
}

impl GitAuthorization {
    pub fn redirect(url: impl Into<String>) -> Self {
        GitAuthorization {
            status: GitAuthorizationStatus::Redirect,
            url: Some(url.into()),
            method: Some(GitRedirectMethod::Get),
            fields: BTreeMap::new(),
            connection: None,
        }
    }

    pub fn form(url: impl Into<String>, fields: BTreeMap<String, String>) -> Self {
        GitAuthorization { method: Some(GitRedirectMethod::Post), fields, ..Self::redirect(url) }
    }

    pub fn connected(connection: GitConnectionView) -> Self {
        GitAuthorization {
            status: GitAuthorizationStatus::Connected,
            url: None,
            method: None,
            fields: BTreeMap::new(),
            connection: Some(connection),
        }
    }
}

/// `POST /api/v1/git/connections` — connect an account of a git provider
/// with a personal access token (instead of authorizing it in the browser).
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectGit {
    pub provider: GitProvider,
    /// A personal access token of the account: on GitHub a classic token
    /// with the `repo` scope (or a fine-grained one with read access to
    /// Contents and Metadata), on GitLab one with the `read_api` and
    /// `read_repository` scopes.
    pub token: String,
    /// Web URL of a self-hosted instance (GitHub Enterprise Server, GitLab
    /// self-managed), e.g. `https://gitlab.example.com`. Default:
    /// `https://github.com` / `https://gitlab.com`.
    pub base_url: Option<String>,
}

impl std::fmt::Debug for ConnectGit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectGit")
            .field("provider", &self.provider)
            .field("token", &"***")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Whether a git connection can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitConnectionStatus {
    /// Repositories are listed and cloned through it.
    Connected,
    /// Its authorization was started in the browser but not finished:
    /// resume it with `POST /api/v1/git/authorize` and its `connection_id`.
    Pending,
}

/// A git connection as returned by the API. Its secrets (tokens, an app's
/// private key and client secret) are never returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GitConnectionView {
    pub id: String,
    pub provider: GitProvider,
    /// Web URL of the provider instance, e.g. `https://github.com`.
    pub base_url: String,
    /// Where its tokens come from.
    pub auth: GitAuth,
    pub status: GitConnectionStatus,
    /// Login of the account. Empty while an OAuth application waits for
    /// its first authorization.
    pub account: String,
    /// Display name of the account, if it has one.
    pub account_name: Option<String>,
    /// The end of a personal access token (`…a1b2`), to tell tokens apart.
    /// `null` for the other kinds.
    pub token_hint: Option<String>,
    /// Scopes of the token, when the provider reports them (empty otherwise).
    pub scopes: Vec<String>,
    /// When a personal access token expires, when the provider reports it.
    /// `null` for the other kinds: their tokens are renewed.
    pub token_expires_at: Option<DateTime<Utc>>,
    /// Id of the OAuth application or GitHub App (not a secret).
    pub client_id: Option<String>,
    /// GitHub App: its URL name.
    pub app_slug: Option<String>,
    /// GitHub App: its page.
    pub app_url: Option<String>,
    /// GitHub App: where the account chooses the repositories it may read.
    pub manage_url: Option<String>,
    /// GitHub App: `all` or `selected` repositories of the account.
    pub repository_selection: Option<String>,
    /// Names of the services whose repository is cloned with this
    /// connection.
    pub services: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A repository a git connection can access.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GitRepository {
    /// The provider's id of the repository.
    pub id: String,
    /// `owner/name` (GitLab: the full path, subgroups included).
    pub full_name: String,
    pub name: String,
    /// The user, organization or group that owns it.
    pub owner: String,
    /// Not public (GitLab: `private` or `internal`).
    pub private: bool,
    pub archived: bool,
    /// `None` for a repository without commits.
    pub default_branch: Option<String>,
    /// The https clone URL: use it as a service's `repo_url`.
    pub clone_url: String,
    /// The repository's page on the provider.
    pub web_url: String,
    pub description: Option<String>,
    /// The last push (GitHub) or activity (GitLab).
    pub updated_at: Option<DateTime<Utc>>,
}

/// `GET /api/v1/git/connections/{id}/repositories`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GitRepositoryList {
    /// Most recently updated first.
    pub repositories: Vec<GitRepository>,
    /// The account can access more repositories than listed (the listing
    /// stops at 1000).
    pub truncated: bool,
}

/// `GET /api/v1/git/branches?repo_url=` — the branches of a repository, as
/// its remote lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GitBranches {
    /// The branch the repository's `HEAD` points to, when it says.
    pub default_branch: Option<String>,
    /// Every branch: the default one first, then by name. Empty for a
    /// repository without commits.
    pub branches: Vec<String>,
    /// Id of the git connection the repository was read with; `null` when
    /// it was read without one.
    pub connection_id: Option<String>,
}

/// `POST /api/v1/blueprints/apply` (JSON body) — or send the raw YAML with
/// `Content-Type: application/yaml` and `?dry_run=true`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyBlueprint {
    /// Contents of `ferry.yaml` / `render.yaml`.
    pub yaml: String,
    #[serde(default)]
    pub dry_run: bool,
}

/// One planned/performed change of a blueprint apply.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

impl From<&crate::Error> for ApiErrorBody {
    fn from(e: &crate::Error) -> Self {
        ApiErrorBody { error: ApiErrorDetail { code: e.code().to_string(), message: e.to_string() } }
    }
}

#[cfg(test)]
mod schema_tests {
    use utoipa::PartialSchema;

    use super::*;
    use crate::models::{DeploySource, ServiceType};

    fn json<T: PartialSchema>() -> String {
        serde_json::to_string(&T::schema()).unwrap_or_default()
    }

    #[test]
    fn openapi_schemas_are_generated() {
        let st = json::<ServiceType>();
        assert!(st.contains("\"web_service\"") && st.contains("\"cron_job\"") && !st.contains("\"web\""), "{st}");
        let view = json::<ServiceView>();
        assert!(view.contains("deploy_hook_path") && view.contains("Service"), "{view}");
        let source = json::<DeploySource>();
        assert!(source.contains("kind") && source.contains("archive"), "{source}");
        let create = json::<CreateService>();
        assert!(create.contains("\"type\""), "{create}");
    }
}
