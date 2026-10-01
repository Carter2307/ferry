/**
 * TypeScript mirror of the Ferry HTTP API wire format — exact field names
 * (snake_case) of `crates/ferry-core/src/{dto,models,logs}.rs`.
 *
 * Timestamps are RFC 3339 strings (`chrono::DateTime<Utc>`). `Option<T>`
 * serializes as `T | null`; fields marked `?` may be absent entirely.
 */

/** RFC 3339 timestamp, e.g. `2026-09-26T14:06:22.721848Z`. */
export type Timestamp = string

// ---------------------------------------------------------------------------
// Enums (models.rs `str_enum!` — canonical wire names)
// ---------------------------------------------------------------------------

export const SERVICE_TYPES = ['web_service', 'private_service', 'background_worker', 'cron_job', 'static_site'] as const
export type ServiceType = (typeof SERVICE_TYPES)[number]

export const RUNTIMES = ['auto', 'docker', 'image', 'node', 'python', 'go', 'rust', 'ruby', 'static'] as const
export type Runtime = (typeof RUNTIMES)[number]

export const DEPLOY_STATUSES = [
  'queued',
  'building',
  'deploying',
  'live',
  'deactivated',
  'build_failed',
  'deploy_failed',
  'canceled',
] as const
export type DeployStatus = (typeof DEPLOY_STATUSES)[number]

export const DEPLOY_TRIGGERS = [
  'create',
  'manual',
  'webhook',
  'deploy_hook',
  'blueprint',
  'rollback',
  'restart',
  'env_change',
  'upload',
] as const
export type DeployTrigger = (typeof DEPLOY_TRIGGERS)[number]

export const JOB_STATUSES = ['pending', 'running', 'succeeded', 'failed', 'canceled'] as const
export type JobStatus = (typeof JOB_STATUSES)[number]

export const JOB_TRIGGERS = ['schedule', 'manual'] as const
export type JobTrigger = (typeof JOB_TRIGGERS)[number]

export const DATASTORE_KINDS = ['postgres', 'redis'] as const
export type DatastoreKind = (typeof DATASTORE_KINDS)[number]

export const DATASTORE_STATUSES = ['creating', 'available', 'failed'] as const
export type DatastoreStatus = (typeof DATASTORE_STATUSES)[number]

export const SERVICE_STATES = ['not_deployed', 'deploying', 'live', 'failed', 'suspended', 'degraded'] as const
export type ServiceState = (typeof SERVICE_STATES)[number]

export const GIT_PROVIDERS = ['github', 'gitlab'] as const
export type GitProvider = (typeof GIT_PROVIDERS)[number]

/** Where the tokens of a git connection come from. */
export type GitAuth = 'github_app' | 'oauth' | 'token'

/** Where a service's code comes from (derived client-side, see `sourceKind`). */
export type SourceKind = 'git' | 'image' | 'upload'

export type LogStreamKind = 'stdout' | 'stderr' | 'system'

// ---------------------------------------------------------------------------
// Models (models.rs)
// ---------------------------------------------------------------------------

export interface Service {
  id: string
  name: string
  type: ServiceType
  repo_url: string | null
  branch: string
  image: string | null
  runtime: Runtime
  root_dir: string | null
  dockerfile_path: string | null
  build_command: string | null
  start_command: string | null
  publish_dir: string | null
  port: number | null
  health_check_path: string | null
  schedule: string | null
  instances: number
  auto_deploy: boolean
  suspended: boolean
  disk_mount_path: string | null
  /**
   * Memory limit of each instance (and of the service's jobs), in MiB.
   * `null` = the server default. Takes effect with the next deploy or restart.
   */
  memory_limit_mb: number | null
  /** CPU limit of each instance (and jobs), in CPUs (`0.5` = half a core). `null` = the server default. */
  cpu_limit: number | null
  custom_domains: string[]
  deploy_hook_key: string
  live_deploy_id: string | null
  created_at: Timestamp
  updated_at: Timestamp
}

export type DeploySource =
  | { kind: 'git'; repo_url: string; branch: string; commit: string | null }
  | { kind: 'archive'; path: string }
  | { kind: 'image'; image: string }
  | { kind: 'reuse'; image: string; from_deploy: string | null }

export interface Deploy {
  id: string
  service_id: string
  status: DeployStatus
  trigger: DeployTrigger
  source: DeploySource
  commit_sha: string | null
  commit_message: string | null
  image: string | null
  port: number | null
  error: string | null
  created_at: Timestamp
  started_at: Timestamp | null
  finished_at: Timestamp | null
}

export interface JobRun {
  id: string
  service_id: string
  trigger: JobTrigger
  command: string | null
  image: string | null
  status: JobStatus
  exit_code: number | null
  error: string | null
  created_at: Timestamp
  started_at: Timestamp | null
  finished_at: Timestamp | null
}

export interface Datastore {
  id: string
  name: string
  kind: DatastoreKind
  version: string
  status: DatastoreStatus
  username: string
  password: string
  database: string | null
  host_port: number | null
  /** Memory limit of the container, in MiB. `null` = the server default. */
  memory_limit_mb: number | null
  /** CPU limit of the container, in CPUs. `null` = the server default. */
  cpu_limit: number | null
  error: string | null
  created_at: Timestamp
  updated_at: Timestamp
}

export interface EnvVar {
  key: string
  value: string
}

export interface EnvGroup {
  id: string
  name: string
  created_at: Timestamp
  updated_at: Timestamp
}

// ---------------------------------------------------------------------------
// Logs (logs.rs)
// ---------------------------------------------------------------------------

export interface LogLine {
  ts: Timestamp
  stream: LogStreamKind
  /** Short container id (runtime logs only). */
  instance?: string | null
  line: string
}

// ---------------------------------------------------------------------------
// DTOs (dto.rs)
// ---------------------------------------------------------------------------

/** `GET /api/v1/info` */
export interface ServerInfo {
  version: string
  base_domain: string
  proxy_url: string
  tls_enabled: boolean
  dashboard_url: string | null
  github_webhook_enabled: boolean
  docker_version: string | null
  /** Memory limit (MiB) of containers that set none. 0 = unlimited. */
  default_memory_limit_mb: number
  /** CPU limit (CPUs) of containers that set none. 0 = unlimited. */
  default_cpu_limit: number
  /** CPUs of the Docker host, if known. */
  docker_cpus: number | null
  /** Total memory of the Docker host (bytes), if known. */
  docker_memory_bytes: number | null
}

/** A service as returned by the API (flattened `Service` + computed fields). */
export interface ServiceView extends Service {
  state: ServiceState
  url: string | null
  hosts: string[]
  internal_host: string
  internal_port: number | null
  env_groups: string[]
  latest_deploy: Deploy | null
  deploy_hook_path: string
}

/** `POST /api/v1/services` */
export interface CreateService {
  name: string
  type?: ServiceType | null
  repo_url?: string | null
  branch?: string | null
  image?: string | null
  runtime?: Runtime | null
  root_dir?: string | null
  dockerfile_path?: string | null
  build_command?: string | null
  start_command?: string | null
  publish_dir?: string | null
  port?: number | null
  health_check_path?: string | null
  schedule?: string | null
  instances?: number | null
  auto_deploy?: boolean | null
  disk_mount_path?: string | null
  /** Memory limit per instance, in MiB (default: the server default). */
  memory_limit_mb?: number | null
  /** CPU limit per instance, in CPUs (default: the server default). */
  cpu_limit?: number | null
  custom_domains?: string[] | null
  env?: EnvVar[] | null
  env_groups?: string[] | null
  deploy?: boolean | null
}

/**
 * `PATCH /api/v1/services/{id}` — empty string clears optional strings, port 0 = auto,
 * `memory_limit_mb` / `cpu_limit` 0 = back to the server default (next deploy).
 */
export interface UpdateService {
  repo_url?: string | null
  branch?: string | null
  image?: string | null
  runtime?: Runtime | null
  root_dir?: string | null
  dockerfile_path?: string | null
  build_command?: string | null
  start_command?: string | null
  publish_dir?: string | null
  port?: number | null
  health_check_path?: string | null
  schedule?: string | null
  instances?: number | null
  auto_deploy?: boolean | null
  suspended?: boolean | null
  disk_mount_path?: string | null
  memory_limit_mb?: number | null
  cpu_limit?: number | null
  custom_domains?: string[] | null
}

/** `POST /api/v1/services/{id}/deploys` */
export interface TriggerDeploy {
  commit?: string | null
  clear_cache?: boolean
}

/** `POST /api/v1/services/{id}/rollback` */
export interface RollbackRequest {
  deploy_id: string
}

/** `POST /api/v1/services/{id}/scale` */
export interface ScaleRequest {
  instances: number
}

/** `PUT …/env` */
export interface ReplaceEnv {
  vars: EnvVar[]
}

/** `PATCH …/env` */
export interface PatchEnv {
  set?: EnvVar[]
  unset?: string[]
}

/** `POST /api/v1/services/{id}/domains` */
export interface DomainRequest {
  domain: string
}

/** `POST /api/v1/services/{id}/jobs` */
export interface RunJobRequest {
  command?: string | null
}

/** `POST /api/v1/services/{id}/env-groups` */
export interface LinkEnvGroup {
  group: string
}

export interface InstanceStatus {
  container_id: string
  name: string
  deploy_id: string | null
  /** Docker state: created, running, restarting, exited, dead… */
  state: string
  host_port: number | null
  started_at: string | null
  restart_count: number | null
  cpu_percent: number | null
  memory_bytes: number | null
  /** The container's configured memory limit (bytes); `null` when unlimited. */
  memory_limit_bytes: number | null
  /** The container's CPU limit (CPUs); `null` when unlimited. */
  cpu_limit: number | null
  /** The kernel killed the container's process for exceeding its memory limit (its last exit). */
  oom_killed: boolean
  /** Exit code of the last exit, for exited / restarting containers. */
  exit_code: number | null
}

/** `GET /api/v1/services/{id}/status` */
export interface RuntimeStatus {
  service_id: string
  state: ServiceState
  desired_instances: number
  instances: InstanceStatus[]
}

/** `POST /api/v1/datastores` */
export interface CreateDatastore {
  name: string
  kind: DatastoreKind
  version?: string | null
  database?: string | null
  username?: string | null
  /** Memory limit, in MiB (default: the server default). */
  memory_limit_mb?: number | null
  /** CPU limit, in CPUs (default: the server default). */
  cpu_limit?: number | null
}

/**
 * `PATCH /api/v1/datastores/{id}` — resource limits. 0 = back to the server
 * default. Applied to the running container in place (no restart).
 */
export interface UpdateDatastore {
  memory_limit_mb?: number | null
  cpu_limit?: number | null
}

export interface DatastoreView extends Datastore {
  internal_host: string
  internal_port: number
  internal_url: string
  external_url: string | null
}

/** `POST /api/v1/env-groups` */
export interface CreateEnvGroup {
  name: string
  vars?: EnvVar[]
}

export interface EnvGroupView extends EnvGroup {
  vars: EnvVar[]
  /** Names of linked services. */
  services: string[]
}

/**
 * `POST /api/v1/git/authorize` — start (or resume, with `connection_id`) the
 * authorization of an account on the provider's own pages.
 */
export interface AuthorizeGit {
  provider?: GitProvider | null
  /** Web URL of a self-hosted instance; default github.com / gitlab.com. */
  base_url?: string | null
  /** Resume a pending connection, or authorize a connection again. */
  connection_id?: string | null
  /** Where the provider sends the browser back: the dashboard's `/git/callback` page. */
  redirect_uri: string
  /** GitHub: register the app in this organization instead of the user's account. */
  organization?: string | null
  /** GitLab: the OAuth application created for this server (needed the first time). */
  client_id?: string | null
  client_secret?: string | null
}

/** `POST /api/v1/git/callback` — the query parameters the provider sent the browser back with. */
export interface GitCallback {
  state: string
  code?: string | null
  installation_id?: number | null
  setup_action?: string | null
  error?: string | null
  error_description?: string | null
}

/** The next step of a browser authorization. */
export interface GitAuthorization {
  /** `redirect`: send the browser to `url`; `connected`: done. */
  status: 'redirect' | 'connected'
  url: string | null
  /** `get`: navigate; `post`: submit a form with `fields`. */
  method: 'get' | 'post' | null
  fields: Record<string, string>
  connection: GitConnectionView | null
}

/** `POST /api/v1/git/connections` — connect the account an access token belongs to. */
export interface ConnectGit {
  provider: GitProvider
  token: string
  /** Web URL of a self-hosted instance; default github.com / gitlab.com. */
  base_url?: string | null
}

/**
 * A GitHub / GitLab account this server is authorized to read the
 * repositories of. Its secrets are never returned.
 */
export interface GitConnectionView {
  id: string
  provider: GitProvider
  /** Web URL of the provider instance, e.g. `https://github.com`. */
  base_url: string
  auth: GitAuth
  /** `pending`: its authorization was started in the browser and not finished. */
  status: 'connected' | 'pending'
  /** Login of the account ('' while an OAuth application waits for its first authorization). */
  account: string
  account_name: string | null
  /** The end of a personal access token (`…a1b2`); `null` for the other kinds. */
  token_hint: string | null
  /** Scopes of the token, when the provider reports them. */
  scopes: string[]
  /** When a personal access token expires; `null` for the other kinds (renewed). */
  token_expires_at: Timestamp | null
  /** Id of the OAuth application or GitHub App (not a secret). */
  client_id: string | null
  /** GitHub App: its URL name and page. */
  app_slug: string | null
  app_url: string | null
  /** GitHub App: where the account chooses the repositories it may read. */
  manage_url: string | null
  /** GitHub App: `all` or `selected` repositories. */
  repository_selection: string | null
  /** Names of the services whose repository is cloned with this connection. */
  services: string[]
  created_at: Timestamp
  updated_at: Timestamp
}

/** A repository a git connection can access. */
export interface GitRepository {
  id: string
  /** `owner/name` (GitLab: the full path, subgroups included). */
  full_name: string
  name: string
  owner: string
  private: boolean
  archived: boolean
  /** `null` for a repository without commits. */
  default_branch: string | null
  /** The https clone URL: a service's `repo_url`. */
  clone_url: string
  web_url: string
  description: string | null
  updated_at: Timestamp | null
}

/** `GET /api/v1/git/connections/{id}/repositories` */
export interface GitRepositoryList {
  /** Most recently updated first. */
  repositories: GitRepository[]
  /** The account can access more repositories than listed. */
  truncated: boolean
}

/** `GET /api/v1/git/branches?repo_url=` — asked from the repository's remote itself. */
export interface GitBranches {
  /** The branch the repository's HEAD points to, when it says. */
  default_branch: string | null
  /** The default branch first, then by name. */
  branches: string[]
  /** The git connection the repository was read with; `null` when read without one. */
  connection_id: string | null
}

/** `POST /api/v1/blueprints/apply` */
export interface ApplyBlueprint {
  yaml: string
  dry_run?: boolean
}

export type BlueprintResource = 'service' | 'datastore' | 'env_group'
export type BlueprintActionKind = 'create' | 'update' | 'unchanged'

export interface BlueprintAction {
  resource: BlueprintResource | (string & {})
  name: string
  action: BlueprintActionKind | (string & {})
  changes: string[]
}

export interface BlueprintResult {
  dry_run: boolean
  actions: BlueprintAction[]
  deploys: Deploy[]
  warnings: string[]
}

/** Error body: `{"error": {"code": "not_found", "message": "…"}}`. */
// ---------------------------------------------------------------------------
// Accounts (DESIGN.md §20)

/** How a request is authenticated: the dashboard's session, or an API token. */
export type AuthKind = 'session' | 'token'

/** The account the dashboard is signed in to. */
export interface UserView {
  id: string
  email: string
  created_at: Timestamp
}

/** `GET /api/v1/auth/status` */
export interface AuthStatus {
  /** The server has no account yet: it is created first. */
  setup_required: boolean
  auth: AuthKind | null
  user: UserView | null
}

/** `POST /api/v1/auth/setup` */
export interface SetupAccount {
  email: string
  password: string
  /** The setup code of the link ferryd prints while the server has no account. */
  code: string
}

/** `POST /api/v1/auth/login` */
export interface Login {
  email: string
  password: string
}

/** `POST /api/v1/auth/password` */
export interface ChangePassword {
  current_password: string
  new_password: string
}

/** A browser signed in to the dashboard. */
export interface SessionView {
  id: string
  user_agent: string | null
  created_at: Timestamp
  last_used_at: Timestamp
  expires_at: Timestamp
  /** The session of this browser. */
  current: boolean
}

/** A named API token. The token itself is only in the answer that creates it. */
export interface ApiTokenView {
  id: string
  name: string
  /** The last characters of the token. */
  hint: string
  created_at: Timestamp
  last_used_at: Timestamp | null
  expires_at: Timestamp | null
}

/** `POST /api/v1/auth/tokens` */
export interface CreateApiToken {
  name: string
  expires_in_days?: number | null
}

/** Answer of `POST /api/v1/auth/tokens`: the only time the token is shown. */
export interface CreatedApiToken {
  token: string
  api_token: ApiTokenView
}

export type CliLoginStatus = 'pending' | 'approved' | 'denied'

/** `GET /api/v1/auth/cli/{id}` — a `ferry login` waiting for its approval. */
export interface CliLoginView {
  id: string
  /** Who asks, e.g. `ada@laptop`. */
  name: string
  /** What the terminal shows too. */
  code: string
  status: CliLoginStatus
  created_at: Timestamp
  expires_at: Timestamp
}

export interface ApiErrorBody {
  error: ApiErrorDetail
}

export interface ApiErrorDetail {
  code: string
  message: string
}

// ---------------------------------------------------------------------------
// Change feed (`GET /api/v1/events`, event `change`)
// ---------------------------------------------------------------------------

export type ChangeKind = 'service' | 'deploy' | 'datastore' | 'env_group' | 'job' | 'git_connection'
export type ChangeAction = 'created' | 'updated' | 'deleted'

export interface ChangeEvent {
  kind: ChangeKind
  id: string
  service_id: string | null
  action: ChangeAction
}

// ---------------------------------------------------------------------------
// Derived helpers (pure, mirror models.rs logic)
// ---------------------------------------------------------------------------

/** `Service::source_kind` */
export function sourceKind(s: Pick<Service, 'image' | 'repo_url'>): SourceKind {
  if (s.image) return 'image'
  if (s.repo_url) return 'git'
  return 'upload'
}

/** `DeployStatus::is_active` */
export function isDeployActive(status: DeployStatus): boolean {
  return status === 'queued' || status === 'building' || status === 'deploying'
}

/** `DeployStatus::is_failed` */
export function isDeployFailed(status: DeployStatus): boolean {
  return status === 'build_failed' || status === 'deploy_failed'
}

/** `JobStatus::is_terminal` */
export function isJobTerminal(status: JobStatus): boolean {
  return status === 'succeeded' || status === 'failed' || status === 'canceled'
}

/** `Service::is_public_http` */
export function isPublicHttp(type: ServiceType): boolean {
  return type === 'web_service' || type === 'static_site'
}

/** `Service::listens` */
export function listensOnPort(type: ServiceType): boolean {
  return type === 'web_service' || type === 'private_service' || type === 'static_site'
}

/** `Service::is_long_running` */
export function isLongRunning(type: ServiceType): boolean {
  return type !== 'cron_job'
}
