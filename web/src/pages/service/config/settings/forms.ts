/**
 * Pure form models for the service Settings page: server value → form
 * state, client-side validation mirroring `ferry-core/src/validate.rs`, and
 * form → minimal `UpdateService` patch (only changed fields; switching the
 * source clears the other one in the same request, as the API requires).
 */

import {
  listensOnPort,
  sourceKind,
  type Runtime,
  type ServiceType,
  type ServiceView,
  type SourceKind,
  type UpdateService,
} from '@/lib/api/types'

export type FormErrors<F> = Partial<Record<keyof F, string>>

// ---------------------------------------------------------------------------
// Build & deploy
// ---------------------------------------------------------------------------

export interface BuildForm {
  source: SourceKind
  repo_url: string
  branch: string
  image: string
  /** Build runtime (git / upload sources; `image` is implied by an image source). */
  runtime: Exclude<Runtime, 'image'>
  root_dir: string
  dockerfile_path: string
  build_command: string
  start_command: string
  publish_dir: string
  /** '' = auto. */
  port: string
  health_check_path: string
  auto_deploy: boolean
}

export const BUILD_RUNTIMES: readonly Exclude<Runtime, 'image'>[] = [
  'auto',
  'docker',
  'node',
  'python',
  'go',
  'rust',
  'ruby',
  'static',
]

type BuildSource = Pick<
  ServiceView,
  | 'repo_url'
  | 'branch'
  | 'image'
  | 'runtime'
  | 'root_dir'
  | 'dockerfile_path'
  | 'build_command'
  | 'start_command'
  | 'publish_dir'
  | 'port'
  | 'health_check_path'
  | 'auto_deploy'
>

export function buildFormFrom(s: BuildSource): BuildForm {
  return {
    source: sourceKind(s),
    repo_url: s.repo_url ?? '',
    branch: s.branch,
    image: s.image ?? '',
    runtime: s.runtime === 'image' ? 'auto' : s.runtime,
    root_dir: s.root_dir ?? '',
    dockerfile_path: s.dockerfile_path ?? '',
    build_command: s.build_command ?? '',
    start_command: s.start_command ?? '',
    publish_dir: s.publish_dir ?? '',
    port: s.port === null ? '' : String(s.port),
    health_check_path: s.health_check_path ?? '',
    auto_deploy: s.auto_deploy,
  }
}

/** Which Build & deploy fields apply to a service type + source. */
export function buildFields(type: ServiceType, f: Pick<BuildForm, 'source' | 'runtime'>) {
  const builds = f.source !== 'image'
  return {
    repo: f.source === 'git',
    image: f.source === 'image',
    runtime: builds,
    rootDir: builds,
    dockerfilePath: builds && (f.runtime === 'docker' || f.runtime === 'auto'),
    buildCommand: builds && f.runtime !== 'docker',
    startCommand: type !== 'static_site',
    publishDir: builds && (type === 'static_site' || f.runtime === 'static'),
    port: listensOnPort(type),
    healthCheck: listensOnPort(type),
    autoDeploy: f.source === 'git',
  }
}

const hasWhitespace = (s: string) => /\s/.test(s)
const hasParent = (p: string) => p.split(/[/\\]/).some((seg) => seg === '..')

/** `validate::repo_url` (simplified). */
export function repoUrlError(url: string): string | null {
  const u = url.trim()
  if (!u) return 'Enter a repository URL.'
  if (hasWhitespace(u)) return 'The URL must not contain spaces.'
  if (u.startsWith('-')) return 'The URL must not start with “-”.'
  if (u.includes('::')) return 'Git remote-helper transports (ext::, fd::…) are not supported.'
  const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(u)
  if (scheme) {
    const s = (scheme[1] ?? '').toLowerCase()
    if (!['https', 'http', 'ssh', 'git', 'file'].includes(s)) return 'Use https://, ssh://, git://, file:// or an absolute path.'
    if (u.length === scheme[0].length) return 'The URL is incomplete.'
    if (s === 'file' && !u.slice(7).startsWith('/')) return 'file:// URLs must use an absolute path (file:///path).'
    return null
  }
  if (u.startsWith('/')) return hasParent(u) ? '“..” is not allowed in local paths.' : null
  if (/^[^@/\s]+@[^:/\s]+:.+/.test(u)) return null
  return 'Use https://, ssh://, git@host:path or an absolute path (relative paths are not allowed).'
}

/** `validate::branch` */
export function branchError(b: string): string | null {
  const v = b.trim()
  if (!v) return 'Enter a branch.'
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
    /[\s~^:?*[\\]/.test(v)
  return bad ? `“${v}” is not a valid branch name.` : null
}

function relativePathError(p: string, allowParent: boolean): string | null {
  const v = p.trim()
  if (!v) return null
  if (v.startsWith('/') || v.startsWith('-') || (!allowParent && hasParent(v))) {
    return 'Use a relative path inside the repository.'
  }
  return null
}

export function portError(p: string): string | null {
  const v = p.trim()
  if (!v) return null
  if (!/^\d+$/.test(v)) return 'Enter a number, or leave empty for auto.'
  const n = Number(v)
  if (n < 1 || n > 65535) return 'Use a port between 1 and 65535 (empty = auto).'
  return null
}

export function healthPathError(p: string): string | null {
  const v = p.trim()
  if (!v) return null
  if (!v.startsWith('/') || hasWhitespace(v)) return 'Start with “/”, without spaces (e.g. /healthz).'
  return null
}

export function validateBuildForm(f: BuildForm, type: ServiceType): FormErrors<BuildForm> {
  const show = buildFields(type, f)
  const e: FormErrors<BuildForm> = {}
  const set = (k: keyof BuildForm, msg: string | null) => {
    if (msg) e[k] = msg
  }
  if (show.repo) {
    set('repo_url', repoUrlError(f.repo_url))
    set('branch', branchError(f.branch))
  }
  if (show.image) {
    const img = f.image.trim()
    set('image', !img ? 'Enter an image reference.' : hasWhitespace(img) ? 'The image must not contain spaces.' : null)
  }
  if (show.rootDir) set('root_dir', relativePathError(f.root_dir, false))
  if (show.dockerfilePath) set('dockerfile_path', relativePathError(f.dockerfile_path, true))
  if (show.publishDir) set('publish_dir', relativePathError(f.publish_dir, false))
  if (show.port) set('port', portError(f.port))
  if (show.healthCheck) set('health_check_path', healthPathError(f.health_check_path))
  return e
}

const TEXT_FIELDS = [
  'root_dir',
  'dockerfile_path',
  'build_command',
  'start_command',
  'publish_dir',
  'health_check_path',
] as const

/**
 * Minimal PATCH body turning `initial` into `next`. Fields that don't apply
 * to the chosen source keep their stored value (they are hidden, not
 * cleared). Empty strings clear optional fields; port '' → 0 (auto).
 */
export function buildPatch(initial: BuildForm, next: BuildForm, type: ServiceType): UpdateService {
  const show = buildFields(type, next)
  const patch: UpdateService = {}
  const was = initial.source

  // Source. The API refuses a switch unless the old source is cleared in the same request.
  if (next.source === 'git') {
    const repo = next.repo_url.trim()
    if (was !== 'git' || repo !== initial.repo_url.trim()) patch.repo_url = repo
    if (was === 'image') patch.image = ''
    const branch = next.branch.trim()
    if (branch !== initial.branch.trim()) patch.branch = branch
  } else if (next.source === 'image') {
    const image = next.image.trim()
    if (was !== 'image' || image !== initial.image.trim()) patch.image = image
    if (was === 'git') patch.repo_url = ''
  } else {
    if (was === 'git') patch.repo_url = ''
    if (was === 'image') patch.image = ''
  }

  // Runtime (an image source implies runtime `image`; the server sets it).
  if (show.runtime) {
    const initialRuntime: Runtime = was === 'image' ? 'image' : initial.runtime
    if (next.runtime !== initialRuntime) patch.runtime = next.runtime
  }

  const applies: Record<(typeof TEXT_FIELDS)[number], boolean> = {
    root_dir: show.rootDir,
    dockerfile_path: show.dockerfilePath,
    build_command: show.buildCommand,
    start_command: show.startCommand,
    publish_dir: show.publishDir,
    health_check_path: show.healthCheck,
  }
  for (const k of TEXT_FIELDS) {
    if (!applies[k]) continue
    const v = next[k].trim()
    if (v !== initial[k].trim()) patch[k] = v
  }

  if (show.port) {
    const p = next.port.trim()
    if (p !== initial.port.trim()) patch.port = p === '' ? 0 : Number(p)
  }
  if (show.autoDeploy && next.auto_deploy !== initial.auto_deploy) patch.auto_deploy = next.auto_deploy
  return patch
}

// ---------------------------------------------------------------------------
// Scaling
// ---------------------------------------------------------------------------

export const MAX_INSTANCES = 50

export interface ScalingForm {
  instances: string
  disk_mount_path: string
}

export function scalingFormFrom(s: Pick<ServiceView, 'instances' | 'disk_mount_path'>): ScalingForm {
  return { instances: String(s.instances), disk_mount_path: s.disk_mount_path ?? '' }
}

/** Disks are supported on web services, private services and workers. */
export function supportsDisk(type: ServiceType): boolean {
  return type === 'web_service' || type === 'private_service' || type === 'background_worker'
}

export function validateScalingForm(f: ScalingForm, type: ServiceType): FormErrors<ScalingForm> {
  const e: FormErrors<ScalingForm> = {}
  const raw = f.instances.trim()
  const n = Number(raw)
  if (!/^\d+$/.test(raw) || n < 1 || n > MAX_INSTANCES) {
    e.instances = `Use a whole number between 1 and ${MAX_INSTANCES} (suspend the service to stop it).`
  }
  const disk = f.disk_mount_path.trim()
  if (disk && supportsDisk(type)) {
    if (!disk.startsWith('/') || disk === '/' || disk.includes('..')) {
      e.disk_mount_path = 'Use an absolute path other than “/” (e.g. /var/data).'
    } else if (!e.instances && n > 1) {
      e.instances = 'Services with a disk are limited to 1 instance.'
    }
  }
  return e
}

export function scalingPatch(initial: ScalingForm, next: ScalingForm, type: ServiceType): UpdateService {
  const patch: UpdateService = {}
  const n = Number(next.instances.trim())
  if (n !== Number(initial.instances)) patch.instances = n
  if (supportsDisk(type)) {
    const disk = next.disk_mount_path.trim()
    if (disk !== initial.disk_mount_path.trim()) patch.disk_mount_path = disk
  }
  return patch
}

// ---------------------------------------------------------------------------
// Cron schedule
// ---------------------------------------------------------------------------

const CRON_ALIASES = ['@yearly', '@annually', '@monthly', '@weekly', '@daily', '@midnight', '@hourly']

/** `Schedule::parse` shape check (5 fields or a known alias). */
export function scheduleError(expr: string): string | null {
  const v = expr.trim()
  if (!v) return 'Cron jobs need a schedule.'
  if (v.startsWith('@')) {
    return CRON_ALIASES.includes(v.toLowerCase()) ? null : `Unknown alias. Use one of ${CRON_ALIASES.join(', ')}.`
  }
  const fields = v.split(/\s+/)
  if (fields.length !== 5) return 'Use 5 fields: minute hour day-of-month month day-of-week (e.g. */15 * * * *).'
  if (fields.some((f) => !/^[\d*/,\-A-Za-z?LW#]+$/.test(f))) return 'A field contains characters cron doesn’t accept.'
  return null
}

/** A short English description of common schedules (null when not recognised). */
export function describeSchedule(expr: string): string | null {
  const v = expr.trim().toLowerCase()
  const alias: Record<string, string> = {
    '@yearly': 'Once a year, Jan 1 at 00:00 UTC',
    '@annually': 'Once a year, Jan 1 at 00:00 UTC',
    '@monthly': 'On the 1st of every month at 00:00 UTC',
    '@weekly': 'Every Sunday at 00:00 UTC',
    '@daily': 'Every day at 00:00 UTC',
    '@midnight': 'Every day at 00:00 UTC',
    '@hourly': 'Every hour, on the hour',
  }
  if (alias[v]) return alias[v]
  const parts = v.split(/\s+/)
  if (parts.length !== 5) return null
  const [min = '', hour = '', dom = '', mon = '', dow = ''] = parts
  const everyDay = dom === '*' && mon === '*' && dow === '*'
  const pad = (s: string) => s.padStart(2, '0')
  if (min === '*' && hour === '*' && everyDay) return 'Every minute'
  const stepMin = /^\*\/(\d+)$/.exec(min)
  if (stepMin && hour === '*' && everyDay) return `Every ${stepMin[1]} minutes`
  if (/^\d+$/.test(min) && hour === '*' && everyDay) return `Every hour at minute ${Number(min)}`
  const stepHour = /^\*\/(\d+)$/.exec(hour)
  if (/^\d+$/.test(min) && stepHour && everyDay) return `Every ${stepHour[1]} hours at minute ${Number(min)}`
  if (/^\d+$/.test(min) && /^\d+$/.test(hour)) {
    const at = `${pad(hour)}:${pad(min)} UTC`
    if (everyDay) return `Every day at ${at}`
    if (dom === '*' && mon === '*' && dow === '1-5') return `Weekdays at ${at}`
    if (dom === '*' && mon === '*' && /^[0-6]$/.test(dow)) {
      const days = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']
      return `Every ${days[Number(dow)]} at ${at}`
    }
    if (/^\d+$/.test(dom) && mon === '*' && dow === '*') return `On day ${Number(dom)} of every month at ${at}`
  }
  return null
}

// ---------------------------------------------------------------------------
// Custom domains
// ---------------------------------------------------------------------------

/** `validate::domain` → normalized domain, or an error message. */
export function checkDomain(input: string): { domain: string; error: string | null } {
  const d = input.trim().replace(/\.+$/, '').toLowerCase().replace(/^https?:\/\//, '').replace(/\/.*$/, '')
  if (!d) return { domain: d, error: 'Enter a domain.' }
  const labelOk = (l: string) => l.length > 0 && l.length <= 63 && !l.startsWith('-') && !l.endsWith('-') && /^[a-z0-9-]+$/.test(l)
  if (d.length > 253 || !d.includes('.') || !d.split('.').every(labelOk)) {
    return { domain: d, error: `“${d}” is not a valid domain (e.g. app.example.com).` }
  }
  const tld = d.split('.').pop() ?? ''
  if (/^\d+$/.test(tld)) return { domain: d, error: 'IP addresses can’t be used as custom domains.' }
  return { domain: d, error: null }
}

/** Shallow equality for flat form objects. */
export function sameForm<F extends object>(a: F, b: F): boolean {
  const ka = Object.keys(a) as (keyof F)[]
  return ka.length === Object.keys(b).length && ka.every((k) => a[k] === b[k])
}
