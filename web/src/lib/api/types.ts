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

export type ChangeKind = 'service' | 'deploy' | 'datastore' | 'env_group' | 'job'
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
