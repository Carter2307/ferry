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
    ApiToken, Datastore, DatastoreKind, Deploy, Domain, EnvGroup, EnvVar, GitAuth, GitProvider, Runtime, Service,
    ServiceState, ServiceType, User,
};

/// `GET /api/v1/info`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServerInfo {
    pub version: String,
    /// The server's `--base-domain`.
    pub base_domain: String,
    /// The domain of the URL services are shown with: the default domain
    /// (§21). The base domain on a server that connected none.
    #[serde(default)]
    pub default_domain: String,
    /// e.g. `http://localhost:8080` — where the proxy listens.
    pub proxy_url: String,
    pub tls_enabled: bool,
    pub dashboard_url: Option<String>,
    /// `/hooks/github` takes deliveries: a connected GitHub account's app
    /// has a webhook (§18), or the server has a webhook secret of its own.
    pub github_webhook_enabled: bool,
    /// The server was started with a webhook secret of its own
    /// (`--github-webhook-secret`): webhooks added to repositories by hand
    /// are accepted.
    #[serde(default)]
    pub github_webhook_secret_set: bool,
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
    /// GitHub delivers the pushes of the account's repositories to this
    /// server (the app was registered with a webhook, from an address GitHub
    /// can reach): services with auto-deploy deploy on push, with nothing
    /// to set up on a repository. `false` for the other kinds of connection.
    #[serde(default)]
    pub push_events: bool,
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

// ---------------------------------------------------------------------------
// domains and certificates (DESIGN.md §21)

/// `POST /api/v1/domains` — connect a domain: once its DNS points at this
/// server, services are served at `<service>.<name>`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectDomain {
    /// A domain or a subdomain of yours: `example.com`, `apps.example.com`.
    pub name: String,
}

/// `PATCH /api/v1/domains/{id}`
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDomain {
    /// `true` makes it the default domain: the one of the URL every service
    /// is shown with. `false` is refused (make another domain the default).
    pub is_default: Option<bool>,
}

/// A DNS record to create where the zone of a domain is managed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DnsRecord {
    /// `A` (an IPv4 address) or `AAAA` (IPv6).
    #[serde(rename = "type")]
    pub record_type: String,
    /// `*`: every name under the domain, so every service. `@`: the domain
    /// itself.
    pub name: String,
    /// The address this server is reached at from the internet; `null` when
    /// the server could not find it out.
    pub value: Option<String>,
    /// `false` for what only serving the domain itself needs.
    pub required: bool,
}

/// A domain as returned by the API (the stored row plus computed fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DomainView {
    #[serde(flatten)]
    pub domain: Domain,
    /// A name that never leaves this machine or its network (`*.localhost`,
    /// `.test`…): nothing to verify, no certificate.
    pub local: bool,
    /// Whether services are served at `<service>.<name>` now.
    pub served: bool,
    /// The DNS records that point the domain at this server (none for a
    /// local domain).
    pub records: Vec<DnsRecord>,
    /// Where a service is found under this domain, with `<service>` in the
    /// place of its name: `https://<service>.example.com`.
    pub url_pattern: String,
}

/// What the server knows about the certificate of a hostname.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CertificateState {
    /// The server runs without HTTPS (no `--https-addr` and `--acme-email`).
    Disabled,
    /// A local name: no certificate authority issues a certificate for it.
    Local,
    /// None yet: the server asks for one shortly.
    Pending,
    /// Being asked for.
    Issuing,
    /// Served to browsers; renewed before `expires_at`.
    Issued,
    /// The last request failed (`error`); it is made again at `retry_at`.
    Failed,
}

/// `GET /api/v1/certificates` — the certificate of one hostname the proxy
/// routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CertificateView {
    pub host: String,
    /// Name of the service the host is routed to; `null` for the dashboard.
    pub service: Option<String>,
    pub state: CertificateState,
    /// `issued`: when the certificate expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// `failed`: what the certificate authority (or reaching it) answered.
    pub error: Option<String>,
    /// `failed`: when the server asks again.
    pub retry_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// accounts (DESIGN.md §20)

/// How a request is authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// The session cookie of the dashboard.
    Session,
    /// An API token (`Authorization: Bearer`).
    Token,
}

/// The account the dashboard is signed in to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UserView {
    pub id: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
}

impl From<&User> for UserView {
    fn from(u: &User) -> Self {
        UserView { id: u.id.clone(), email: u.email.clone(), created_at: u.created_at }
    }
}

/// `GET /api/v1/auth/status` — what the dashboard needs to know before
/// anything else: is there an account, and is this browser signed in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuthStatus {
    /// The server has no account yet: it must be created first
    /// (`POST /api/v1/auth/setup`).
    pub setup_required: bool,
    /// How this request is authenticated; `null` when it isn't.
    pub auth: Option<AuthKind>,
    /// The account, for a request with a session.
    pub user: Option<UserView>,
}

/// `POST /api/v1/auth/setup` — create the administrator's account of a
/// server that has none.
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupAccount {
    pub email: String,
    /// At least 8 characters.
    pub password: String,
    /// The setup code: the `code` of the link `ferryd` prints while the
    /// server has no account (also in `<data-dir>/setup_code`).
    pub code: String,
}

/// `POST /api/v1/auth/login`
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Login {
    pub email: String,
    pub password: String,
}

/// `POST /api/v1/auth/password`
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangePassword {
    pub current_password: String,
    /// At least 8 characters.
    pub new_password: String,
}

/// Bodies with a password say nothing about it in logs.
macro_rules! opaque_debug {
    ($($name:ident),+) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )+};
}
opaque_debug!(SetupAccount, Login, ChangePassword, CreatedApiToken, CliLoginStarted, CliLoginPoll, CliLoginResult);

/// A browser signed in to the dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionView {
    pub id: String,
    /// The browser, as it named itself when it signed in.
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    /// A session ends 30 days after it was last used.
    pub expires_at: DateTime<Utc>,
    /// The session of this request.
    pub current: bool,
}

/// A named API token. The token itself is only in the answer that creates it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApiTokenView {
    pub id: String,
    pub name: String,
    /// The last characters of the token.
    pub hint: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// `null`: valid until it is revoked.
    pub expires_at: Option<DateTime<Utc>>,
}

impl From<&ApiToken> for ApiTokenView {
    fn from(t: &ApiToken) -> Self {
        ApiTokenView {
            id: t.id.clone(),
            name: t.name.clone(),
            hint: t.hint.clone(),
            created_at: t.created_at,
            last_used_at: t.last_used_at,
            expires_at: t.expires_at,
        }
    }
}

/// `POST /api/v1/auth/tokens`
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApiToken {
    /// What the token is for, e.g. `CI` or `laptop` (1 to 100 characters).
    pub name: String,
    /// Days until the token expires (1 to 3650). Default: it doesn't.
    pub expires_in_days: Option<u32>,
}

/// Answer of `POST /api/v1/auth/tokens`: the only time the token is shown.
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreatedApiToken {
    /// The token, to send as `Authorization: Bearer <token>`. Keep it: the
    /// server only stores its digest.
    pub token: String,
    pub api_token: ApiTokenView,
}

/// `POST /api/v1/auth/cli` — a terminal asks to be connected (`ferry login`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartCliLogin {
    /// Who asks, e.g. `ada@laptop`: shown on the approval page, and the name
    /// of the API token that approving creates.
    pub name: Option<String>,
}

/// Answer of `POST /api/v1/auth/cli`.
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CliLoginStarted {
    /// Names the request: the approval page is `/cli-login?id=<id>` of the dashboard.
    pub id: String,
    /// Shown by the terminal and by the approval page: the person approving
    /// checks that both match.
    pub code: String,
    /// What the terminal collects the token with. It is never shown.
    pub secret: String,
    /// Seconds until the request expires.
    pub expires_in: u64,
    /// Seconds to wait between two polls.
    pub interval: u64,
}

/// Where a `ferry login` request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CliLoginStatus {
    Pending,
    Approved,
    Denied,
}

/// `GET /api/v1/auth/cli/{id}` — what the approval page shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CliLoginView {
    pub id: String,
    pub name: String,
    pub code: String,
    pub status: CliLoginStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// `POST /api/v1/auth/cli/{id}/token`
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CliLoginPoll {
    /// The `secret` of the answer that started the request.
    pub secret: String,
}

/// Answer of `POST /api/v1/auth/cli/{id}/token`.
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CliLoginResult {
    pub status: CliLoginStatus,
    /// The API token, once: with `approved`, the first time it is collected.
    pub token: Option<String>,
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
