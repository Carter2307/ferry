/**
 * New-service form model: state, which fields apply to a service type /
 * source, client-side validation mirroring `ferry_core::validate`, and the
 * `CreateService` request body (only fields that apply, so the server never
 * sees e.g. a schedule on a web service).
 */

import {
  isLongRunning,
  isPublicHttp,
  listensOnPort,
  type CreateService,
  type EnvVar,
  type Runtime,
  type ServiceType,
} from '@/lib/api/types'
import { DEFAULT_LIMIT, limitError, limitValue, type LimitField } from '@/lib/resources'

export type SourceMode = 'git' | 'image' | 'upload'

/** How the repository of a git source is given: picked from a connected account, or by URL. */
export type RepoMode = 'account' | 'url'

/** A repository picked from a connected GitHub / GitLab account. */
export interface PickedRepository {
  /**
   * The git connection it was listed through. Only for the form: a service
   * names no connection, the server clones a repository with the connection
   * that serves its URL.
   */
  connectionId: string
  fullName: string
  cloneUrl: string
  private: boolean
  defaultBranch: string | null
}

export interface NewServiceForm {
  type: ServiceType
  name: string
  source: SourceMode
  repoMode: RepoMode
  /** The repository picked from a connected account (`repoMode` = account). */
  repository: PickedRepository | null
  /** The repository's URL (`repoMode` = url). */
  repoUrl: string
  branch: string
  image: string
  /** Build runtime for git / upload sources (`image` is implied by an image source). */
  runtime: Exclude<Runtime, 'image'>
  buildCommand: string
  startCommand: string
  rootDir: string
  dockerfilePath: string
  publishDir: string
  port: string
  healthCheckPath: string
  instances: string
  schedule: string
  diskMountPath: string
  domains: string[]
  /** Domain text typed but not yet added as a chip (added on submit when valid). */
  domainDraft: string
  envGroups: string[]
  autoDeploy: boolean
  /** Memory limit per instance / job run (Advanced; default = the server default). */
  memoryLimit: LimitField
  /** CPU limit per instance / job run (Advanced; default = the server default). */
  cpuLimit: LimitField
}

export type FieldKey = keyof NewServiceForm | 'env'
export type FieldErrors = Partial<Record<FieldKey, string>>

export const INITIAL_FORM: NewServiceForm = {
  type: 'web_service',
  name: '',
  source: 'git',
  repoMode: 'account',
  repository: null,
  repoUrl: '',
  branch: 'main',
  image: '',
  runtime: 'auto',
  buildCommand: '',
  startCommand: '',
  rootDir: '',
  dockerfilePath: '',
  publishDir: '',
  port: '',
  healthCheckPath: '',
  instances: '1',
  schedule: '',
  diskMountPath: '',
  domains: [],
  domainDraft: '',
  envGroups: [],
  autoDeploy: true,
  memoryLimit: DEFAULT_LIMIT,
  cpuLimit: DEFAULT_LIMIT,
}

const DRAFT_KEY = 'ferry.new-service.draft'

/**
 * The form as it is kept while the browser is away at GitHub / GitLab
 * (connecting an account takes it to the provider and back). A repository
 * URL that carries credentials is left out: nothing secret is stored.
 */
export function draftOf(form: NewServiceForm): NewServiceForm {
  const withCredentials = /^[a-z][a-z0-9+.-]*:\/\/[^/@]*@/i.test(form.repoUrl.trim())
  return withCredentials ? { ...form, repoUrl: '' } : form
}

/** A stored draft read back: only fields of the expected shape are taken. */
export function formFromDraft(raw: string | null): NewServiceForm | null {
  if (!raw) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return null
  const draft = parsed as Record<string, unknown>
  const out: Record<string, unknown> = { ...INITIAL_FORM }
  for (const [key, initial] of Object.entries(INITIAL_FORM)) {
    const value = draft[key]
    if (value === undefined) continue
    const sameShape =
      key === 'repository'
        ? value === null || (typeof value === 'object' && !Array.isArray(value))
        : Array.isArray(initial)
          ? Array.isArray(value) && value.every((v) => typeof v === 'string')
          : typeof value === typeof initial && value !== null && !Array.isArray(value)
    if (sameShape) out[key] = value
  }
  return out as unknown as NewServiceForm
}

/** `sessionStorage` (this tab only), never throwing. */
function session(): Storage | null {
  try {
    return window.sessionStorage
  } catch {
    return null
  }
}

export function saveDraft(form: NewServiceForm): void {
  try {
    session()?.setItem(DRAFT_KEY, JSON.stringify(draftOf(form)))
  } catch {
    /* storage unavailable or full: the form just isn't kept */
  }
}

export function readDraft(): NewServiceForm | null {
  try {
    return formFromDraft(session()?.getItem(DRAFT_KEY) ?? null)
  } catch {
    return null
  }
}

export function clearDraft(): void {
  try {
    session()?.removeItem(DRAFT_KEY)
  } catch {
    /* storage unavailable */
  }
}

export const MAX_INSTANCES = 50
const RESERVED_NAMES = ['ferry', 'localhost']
const ID_PREFIXES = ['srv', 'dep', 'job', 'dbs', 'evg', 'git']

/** Which settings apply to the current type and source. */
export function visibleFields(f: Pick<NewServiceForm, 'type' | 'source' | 'runtime'>) {
  const built = f.source !== 'image'
  return {
    repo: f.source === 'git',
    image: f.source === 'image',
    runtime: built,
    rootDir: built,
    dockerfilePath: built && (f.runtime === 'docker' || f.runtime === 'auto'),
    buildCommand: built && f.runtime !== 'docker',
    startCommand: f.type !== 'static_site',
    publishDir: f.type === 'static_site' && built,
    port: f.type === 'web_service' || f.type === 'private_service',
    healthCheckPath: listensOnPort(f.type),
    instances: isLongRunning(f.type),
    schedule: f.type === 'cron_job',
    disk: f.type === 'web_service' || f.type === 'private_service' || f.type === 'background_worker',
    domains: isPublicHttp(f.type),
    autoDeploy: f.source === 'git',
    /** Every type runs containers (static sites too), so every type has limits. */
    limits: true,
  }
}

export type VisibleFields = ReturnType<typeof visibleFields>

// ---------------------------------------------------------------------------
// Validators (mirror ferry_core::validate) — return a message or null.
// ---------------------------------------------------------------------------

function looksLikeId(s: string): boolean {
  return ID_PREFIXES.some((p) => s.length === p.length + 21 && s.startsWith(`${p}-`) && /^[0-9a-f]+$/.test(s.slice(p.length + 1)))
}

/** `validate::resource_name`, plus uniqueness against services / datastores we know about. */
export function validateName(name: string, taken: { services: string[]; datastores: string[] } = { services: [], datastores: [] }): string | null {
  const n = name.trim()
  if (!n) return 'Enter a name.'
  if (n.length > 40) return 'Use at most 40 characters.'
  if (!/^[a-z]/.test(n)) return 'Start with a lowercase letter.'
  if (!/^[a-z0-9-]+$/.test(n)) return "Use lowercase letters, digits and '-' only."
  if (n.endsWith('-')) return "Don't end the name with '-'."
  if (RESERVED_NAMES.includes(n)) return `“${n}” is reserved.`
  if (looksLikeId(n)) return 'This looks like a resource id; choose another name.'
  if (taken.services.includes(n)) return `A service named “${n}” already exists.`
  if (taken.datastores.includes(n)) return `A datastore is already named “${n}” (services and datastores share names).`
  return null
}

/** `validate::repo_url` */
export function validateRepoUrl(url: string): string | null {
  const u = url.trim()
  if (!u) return 'Enter a repository URL or an absolute path on the server.'
  if (/[\s\p{Cc}]/u.test(u)) return 'The URL must not contain spaces.'
  if (u.startsWith('-')) return "The URL must not start with '-'."
  if (u.includes('::')) return 'Git remote-helper transports (ext::, fd::…) are not supported.'
  if (u.includes('\\')) return 'The URL must not contain backslashes.'
  const hasParent = (p: string) => p.split('/').some((seg) => seg === '..')
  const idx = u.indexOf('://')
  if (idx >= 0) {
    const scheme = u.slice(0, idx)
    const rest = u.slice(idx + 3)
    if (scheme === 'file') {
      if (!rest.startsWith('/')) return 'file:// URLs must use an absolute path (file:///path).'
      if (hasParent(rest)) return "'..' is not allowed in local paths."
      return null
    }
    if (['https', 'http', 'ssh', 'git'].includes(scheme) && rest) return null
    return 'Use https://, ssh://, git://, file:// or an absolute path.'
  }
  if (u.startsWith('file:')) return 'file URLs must look like file:///absolute/path.'
  if (u.startsWith('/')) return hasParent(u) ? "'..' is not allowed in local paths." : null
  const colon = u.indexOf(':')
  if (colon > 0) {
    const host = u.slice(0, colon)
    const hostName = host.split('@').pop() ?? host
    if (hostName.length > 1 && !host.includes('/') && colon + 1 < u.length) return null
  }
  return 'Local repositories must be given as an absolute path on the server.'
}

/** `validate::branch` (empty = main). */
export function validateBranch(b: string): string | null {
  const v = b.trim()
  if (!v) return null
  const bad =
    v.length > 255 ||
    v.startsWith('-') ||
    v.startsWith('/') ||
    v.endsWith('/') ||
    v.endsWith('.') ||
    v.endsWith('.lock') ||
    v.includes('..') ||
    v.includes('//') ||
    v.includes('@{') ||
    /[\s\p{Cc}~^:?*[\\]/u.test(v)
  return bad ? 'This is not a valid git branch name.' : null
}

export function validateImage(image: string): string | null {
  const v = image.trim()
  if (!v) return 'Enter an image reference, e.g. nginx:alpine or ghcr.io/you/app:1.2.'
  if (/\s/.test(v)) return 'Image references cannot contain spaces.'
  return null
}

/** root_dir / publish_dir (no '..') and dockerfile_path ('..' allowed) must stay relative. */
export function validateRelativePath(p: string, allowParent = false): string | null {
  const v = p.trim()
  if (!v) return null
  if (v.startsWith('/') || v.startsWith('-')) return 'Use a path relative to the repository root.'
  if (!allowParent && v.split(/[/\\]/).some((seg) => seg === '..')) return "'..' is not allowed here."
  return null
}

/** `checks::source_paths`: dockerfile_path (relative to root_dir) must stay inside the checkout. */
export function validateDockerfilePath(dockerfile: string, rootDir: string): string | null {
  const basic = validateRelativePath(dockerfile, true)
  if (basic || !dockerfile.trim()) return basic
  let depth = 0
  for (const seg of [...rootDir.trim().split(/[/\\]/), ...dockerfile.trim().split(/[/\\]/)]) {
    if (seg === '' || seg === '.') continue
    if (seg === '..') {
      depth -= 1
      if (depth < 0) return 'This path points outside the source directory.'
    } else depth += 1
  }
  return null
}

export function validatePort(port: string): string | null {
  const v = port.trim()
  if (!v) return null
  if (!/^\d+$/.test(v)) return 'Enter a number.'
  const n = Number(v)
  return n >= 1 && n <= 65535 ? null : 'Use a port between 1 and 65535.'
}

export function validateHealthPath(p: string): string | null {
  const v = p.trim()
  if (!v) return null
  if (!v.startsWith('/')) return "The path must start with '/'."
  return /\s/.test(v) ? 'The path cannot contain spaces.' : null
}

export function validateInstances(value: string, hasDisk: boolean): string | null {
  const v = value.trim()
  if (!/^\d+$/.test(v)) return 'Enter a whole number.'
  const n = Number(v)
  if (n < 1 || n > MAX_INSTANCES) return `Use between 1 and ${MAX_INSTANCES} instances.`
  if (hasDisk && n > 1) return 'Services with a disk are limited to 1 instance.'
  return null
}

const CRON_ALIASES = ['@yearly', '@annually', '@monthly', '@weekly', '@daily', '@midnight', '@hourly']

/** Shape check of `Schedule::parse` (the server checks the field values). */
export function validateSchedule(expr: string): string | null {
  const v = expr.trim()
  if (!v) return 'Cron jobs need a schedule.'
  if (v.startsWith('@')) {
    return CRON_ALIASES.includes(v.toLowerCase()) ? null : `Supported aliases: ${CRON_ALIASES.join(', ')}.`
  }
  const fields = v.split(/\s+/)
  if (fields.length !== 5) return `Expected 5 fields (minute hour day-of-month month day-of-week), got ${fields.length}.`
  if (!fields.every((f) => /^[0-9A-Za-z*/,\-?LW#]+$/.test(f))) return 'Fields may only use digits, names, * / , and -.'
  return null
}

export function validateMountPath(p: string): string | null {
  const v = p.trim()
  if (!v) return null
  return v.startsWith('/') && v.length > 1 && !v.includes('..') ? null : "Use an absolute path other than '/', e.g. /data."
}

/** `validate::domain` — returns the normalized domain or an error. */
export function normalizeDomain(input: string): { domain: string } | { error: string } {
  const d = input.trim().replace(/\.+$/, '').toLowerCase().replace(/^https?:\/\//, '').replace(/\/.*$/, '')
  const labelOk = (l: string) => l.length > 0 && l.length <= 63 && !l.startsWith('-') && !l.endsWith('-') && /^[a-z0-9-]+$/.test(l)
  if (!d || d.length > 253 || !d.includes('.') || !d.split('.').every(labelOk)) {
    return { error: `“${input.trim()}” is not a valid domain name.` }
  }
  if (/^\d+$/.test(d.split('.').pop() ?? '')) return { error: "IP addresses can't be used as custom domains." }
  return { domain: d }
}

/** Split a typed / pasted list of domains and validate each one. */
export function parseDomains(text: string): { domains: string[] } | { error: string } {
  const domains: string[] = []
  for (const part of text.split(/[\s,]+/).filter(Boolean)) {
    const res = normalizeDomain(part)
    if ('error' in res) return res
    if (!domains.includes(res.domain)) domains.push(res.domain)
  }
  return { domains }
}

/**
 * Every field error for the current state (only fields that apply).
 * `hostCpus` (Docker host CPUs, from `/api/v1/info`) caps the CPU limit.
 */
export function validateForm(
  f: NewServiceForm,
  taken: { services: string[]; datastores: string[] },
  hostCpus?: number | null,
): FieldErrors {
  const v = visibleFields(f)
  const errors: FieldErrors = {}
  const set = (k: FieldKey, msg: string | null) => {
    if (msg) errors[k] = msg
  }
  set('name', validateName(f.name, taken))
  if (v.repo) {
    if (f.repoMode === 'account') set('repository', f.repository ? null : 'Select a repository.')
    else set('repoUrl', validateRepoUrl(f.repoUrl))
    set('branch', validateBranch(f.branch))
  }
  if (v.image) set('image', validateImage(f.image))
  if (v.rootDir) set('rootDir', validateRelativePath(f.rootDir))
  if (v.dockerfilePath) set('dockerfilePath', validateDockerfilePath(f.dockerfilePath, v.rootDir ? f.rootDir : ''))
  if (v.publishDir) set('publishDir', validateRelativePath(f.publishDir))
  if (v.port) set('port', validatePort(f.port))
  if (v.healthCheckPath) set('healthCheckPath', validateHealthPath(f.healthCheckPath))
  if (v.disk) set('diskMountPath', validateMountPath(f.diskMountPath))
  if (v.instances) set('instances', validateInstances(f.instances, v.disk && f.diskMountPath.trim() !== ''))
  if (v.schedule) set('schedule', validateSchedule(f.schedule))
  if (v.limits) {
    set('memoryLimit', limitError('memory', f.memoryLimit))
    set('cpuLimit', limitError('cpu', f.cpuLimit, hostCpus))
  }
  if (v.domains && f.domainDraft.trim()) {
    const parsed = parseDomains(f.domainDraft)
    if ('error' in parsed) set('domains', parsed.error)
  }
  return errors
}

/** Order in which errors are focused (top of the page first). */
export const FIELD_ORDER: FieldKey[] = [
  'type',
  'name',
  'source',
  'repository',
  'repoUrl',
  'branch',
  'image',
  'runtime',
  'buildCommand',
  'startCommand',
  'publishDir',
  'schedule',
  'port',
  'instances',
  'rootDir',
  'dockerfilePath',
  'healthCheckPath',
  'diskMountPath',
  'memoryLimit',
  'cpuLimit',
  'domains',
  'env',
  'envGroups',
]

const opt = (s: string): string | undefined => {
  const t = s.trim()
  return t === '' ? undefined : t
}

/** Request body for `POST /api/v1/services` (fields that don't apply are omitted). */
export function toCreateRequest(f: NewServiceForm, env: EnvVar[]): CreateService {
  const v = visibleFields(f)
  const body: CreateService = { name: f.name.trim(), type: f.type }
  if (v.repo) {
    // A picked repository is just its clone URL: the server clones it with
    // the account connected for that host.
    body.repo_url = f.repoMode === 'account' && f.repository ? f.repository.cloneUrl : f.repoUrl.trim()
    body.branch = opt(f.branch) ?? 'main'
    body.auto_deploy = f.autoDeploy
  }
  if (v.image) body.image = f.image.trim()
  if (v.runtime) body.runtime = f.runtime
  if (v.rootDir) body.root_dir = opt(f.rootDir)
  if (v.dockerfilePath) body.dockerfile_path = opt(f.dockerfilePath)
  if (v.buildCommand) body.build_command = opt(f.buildCommand)
  if (v.startCommand) body.start_command = opt(f.startCommand)
  if (v.publishDir) body.publish_dir = opt(f.publishDir)
  if (v.port && f.port.trim()) body.port = Number(f.port.trim())
  if (v.healthCheckPath) body.health_check_path = opt(f.healthCheckPath)
  if (v.instances) body.instances = Number(f.instances.trim())
  if (v.schedule) body.schedule = f.schedule.trim()
  if (v.disk) body.disk_mount_path = opt(f.diskMountPath)
  if (v.limits) {
    const memory = limitValue('memory', f.memoryLimit)
    const cpu = limitValue('cpu', f.cpuLimit)
    if (memory !== null) body.memory_limit_mb = memory
    if (cpu !== null) body.cpu_limit = cpu
  }
  if (v.domains) {
    const draft = parseDomains(f.domainDraft)
    const domains = [...f.domains, ...('domains' in draft ? draft.domains.filter((d) => !f.domains.includes(d)) : [])]
    if (domains.length > 0) body.custom_domains = domains
  }
  if (env.length > 0) body.env = env
  if (f.envGroups.length > 0) body.env_groups = f.envGroups
  // Services without a source wait for `ferry up`; the others deploy right away.
  body.deploy = f.source !== 'upload'
  // Drop undefined keys so the JSON only carries what the user set.
  return Object.fromEntries(Object.entries(body).filter(([, value]) => value !== undefined)) as CreateService
}

/** Best guess of the field a server validation message is about (for inline display). */
export function fieldForServerError(message: string): FieldKey | null {
  const m = message.toLowerCase()
  const rules: [RegExp, FieldKey][] = [
    [/memory limit/, 'memoryLimit'],
    [/cpu limit|range of cpus/, 'cpuLimit'],
    [/env group/, 'envGroups'],
    [/environment variable|variables total|value of /, 'env'],
    [/domain '/, 'domains'],
    [/custom domain|host '/, 'name'],
    [/repo_url|either repo_url or image/, 'repoUrl'],
    [/branch/, 'branch'],
    [/image/, 'image'],
    [/schedule|cron/, 'schedule'],
    [/root_dir/, 'rootDir'],
    [/dockerfile/, 'dockerfilePath'],
    [/publish_dir/, 'publishDir'],
    [/health/, 'healthCheckPath'],
    [/disk|mount path/, 'diskMountPath'],
    [/instances/, 'instances'],
    [/port/, 'port'],
    [/name/, 'name'],
  ]
  return rules.find(([re]) => re.test(m))?.[1] ?? null
}

export const CRON_PRESETS: { label: string; value: string }[] = [
  { label: 'Every 15 min', value: '*/15 * * * *' },
  { label: 'Hourly', value: '0 * * * *' },
  { label: 'Daily 03:00', value: '0 3 * * *' },
  { label: 'Mondays 09:00', value: '0 9 * * 1' },
]
