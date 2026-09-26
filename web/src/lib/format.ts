/**
 * Pure formatting helpers (relative times, bytes, durations, ids, labels).
 * Every function accepts nullish input and returns a display string.
 */

import type {
  DatastoreKind,
  DeploySource,
  DeployStatus,
  DeployTrigger,
  JobStatus,
  Runtime,
  ServiceState,
  ServiceType,
} from '@/lib/api/types'

const EM_DASH = '—'

function toDate(value: string | number | Date | null | undefined): Date | null {
  if (value === null || value === undefined || value === '') return null
  const d = value instanceof Date ? value : new Date(value)
  return Number.isNaN(d.getTime()) ? null : d
}

/** "just now", "5s ago", "3m ago", "2h ago", "4d ago", then a short date. Future → "in 3m". */
export function relativeTime(
  value: string | number | Date | null | undefined,
  now: number = Date.now(),
): string {
  const d = toDate(value)
  if (!d) return EM_DASH
  const diff = now - d.getTime()
  const abs = Math.abs(diff)
  const future = diff < 0
  const fmt = (n: number, unit: string) => (future ? `in ${n}${unit}` : `${n}${unit} ago`)
  if (abs < 5_000) return 'just now'
  if (abs < 60_000) return fmt(Math.floor(abs / 1000), 's')
  if (abs < 3_600_000) return fmt(Math.floor(abs / 60_000), 'm')
  if (abs < 86_400_000) return fmt(Math.floor(abs / 3_600_000), 'h')
  if (abs < 30 * 86_400_000) return fmt(Math.floor(abs / 86_400_000), 'd')
  return shortDate(d, now)
}

/** "Sep 26" (same year) or "Sep 26, 2025". */
export function shortDate(value: string | number | Date | null | undefined, now: number = Date.now()): string {
  const d = toDate(value)
  if (!d) return EM_DASH
  const sameYear = d.getFullYear() === new Date(now).getFullYear()
  return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) })
}

/** "Sep 26, 2026, 2:27:04 PM" in the viewer's locale-ish English format. */
/**
 * Compact local date + time for tight cells: "Sep 26, 20:44:57" (the year is
 * added when it isn't the current one: "Sep 26 2025, 20:44:57").
 */
export function shortDateTime(value: string | number | Date | null | undefined, now: Date = new Date()): string {
  const d = toDate(value)
  if (!d) return EM_DASH
  const pad = (n: number) => String(n).padStart(2, '0')
  const day = d.toLocaleString('en-US', { month: 'short', day: 'numeric' })
  const year = d.getFullYear() === now.getFullYear() ? '' : ` ${d.getFullYear()}`
  return `${day}${year}, ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

export function dateTime(value: string | number | Date | null | undefined): string {
  const d = toDate(value)
  if (!d) return EM_DASH
  return d.toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
    second: '2-digit',
  })
}

/** "14:27:04.123" — log timestamps (local time, 24h). */
export function logTime(value: string | null | undefined, withMillis = true): string {
  const d = toDate(value)
  if (!d) return ''
  const pad = (n: number, w = 2) => String(n).padStart(w, '0')
  const base = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  return withMillis ? `${base}.${pad(d.getMilliseconds(), 3)}` : base
}

/** Duration in ms → "850ms", "12s", "3m 04s", "1h 02m". */
export function duration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) return EM_DASH
  if (ms < 1000) return `${Math.round(ms)}ms`
  const totalSeconds = Math.floor(ms / 1000)
  if (totalSeconds < 60) return `${totalSeconds}s`
  const h = Math.floor(totalSeconds / 3600)
  const m = Math.floor((totalSeconds % 3600) / 60)
  const s = totalSeconds % 60
  if (h > 0) return `${h}h ${String(m).padStart(2, '0')}m`
  return `${m}m ${String(s).padStart(2, '0')}s`
}

/** Elapsed time between two timestamps; `end` defaults to now (running). */
export function durationBetween(
  start: string | null | undefined,
  end?: string | null,
  now: number = Date.now(),
): string {
  const s = toDate(start)
  if (!s) return EM_DASH
  const e = toDate(end ?? null)
  return duration((e ? e.getTime() : now) - s.getTime())
}

/** 1536 → "1.5 KiB" (binary units, like Docker). */
export function bytes(n: number | null | undefined, digits = 1): string {
  if (n === null || n === undefined || !Number.isFinite(n) || n < 0) return EM_DASH
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let v = n
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  const fixed = i === 0 ? String(Math.round(v)) : v.toFixed(v >= 100 ? 0 : digits)
  return `${fixed} ${units[i]}`
}

/** 12.345 → "12.3%". */
export function percent(n: number | null | undefined, digits = 1): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return EM_DASH
  return `${n.toFixed(digits)}%`
}

/** "dep-fd8a901ce5724b6ea476" → "dep-fd8a901c" (prefix kept, 8 hex chars). */
export function shortId(id: string | null | undefined, len = 8): string {
  if (!id) return EM_DASH
  const dash = id.indexOf('-')
  if (dash > 0 && dash < 6) return id.slice(0, dash + 1 + len)
  return id.slice(0, len)
}

/** Git sha → 7 chars. */
export function shortSha(sha: string | null | undefined): string {
  if (!sha) return EM_DASH
  return sha.slice(0, 7)
}

/** "1 instance" / "3 instances". */
export function plural(n: number, singular: string, pluralForm = `${singular}s`): string {
  return `${n} ${n === 1 ? singular : pluralForm}`
}

/** Remove ANSI escape sequences (colors, cursor moves, OSC links). */
export function stripAnsi(text: string): string {
  // CSI … final byte, OSC … BEL/ST, and lone ESC sequences.
  // eslint-disable-next-line no-control-regex
  return text.replace(/\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\u001b[@-Z\\-_]/g, '')
}

/** Strip scheme from a URL for display: "http://api.localhost:8080" → "api.localhost:8080". */
export function displayUrl(url: string | null | undefined): string {
  if (!url) return EM_DASH
  return url.replace(/^[a-z]+:\/\//i, '').replace(/\/$/, '')
}

/** Last path segment of a repo URL/path: "/srv/git/my-app.git" → "my-app". */
export function repoName(repo: string | null | undefined): string {
  if (!repo) return EM_DASH
  const cleaned = repo.replace(/\/+$/, '').replace(/\.git$/, '')
  const last = cleaned.split(/[/:]/).pop()
  return last || cleaned
}

// ---------------------------------------------------------------------------
// Enum labels
// ---------------------------------------------------------------------------

export const SERVICE_TYPE_LABELS: Record<ServiceType, string> = {
  web_service: 'Web service',
  private_service: 'Private service',
  background_worker: 'Background worker',
  cron_job: 'Cron job',
  static_site: 'Static site',
}

export const RUNTIME_LABELS: Record<Runtime, string> = {
  auto: 'Auto-detect',
  docker: 'Docker',
  image: 'Prebuilt image',
  node: 'Node.js',
  python: 'Python',
  go: 'Go',
  rust: 'Rust',
  ruby: 'Ruby',
  static: 'Static',
}

export const SERVICE_STATE_LABELS: Record<ServiceState, string> = {
  not_deployed: 'Not deployed',
  deploying: 'Deploying',
  live: 'Live',
  failed: 'Failed',
  suspended: 'Suspended',
  degraded: 'Degraded',
}

export const DEPLOY_STATUS_LABELS: Record<DeployStatus, string> = {
  queued: 'Queued',
  building: 'Building',
  deploying: 'Deploying',
  live: 'Live',
  deactivated: 'Deactivated',
  build_failed: 'Build failed',
  deploy_failed: 'Deploy failed',
  canceled: 'Canceled',
}

export const DEPLOY_TRIGGER_LABELS: Record<DeployTrigger, string> = {
  create: 'Service created',
  manual: 'Manual deploy',
  webhook: 'Git push',
  deploy_hook: 'Deploy hook',
  blueprint: 'Blueprint',
  rollback: 'Rollback',
  restart: 'Restart',
  env_change: 'Environment change',
  upload: 'Upload',
}

export const JOB_STATUS_LABELS: Record<JobStatus, string> = {
  pending: 'Pending',
  running: 'Running',
  succeeded: 'Succeeded',
  failed: 'Failed',
  canceled: 'Canceled',
}

export const DATASTORE_KIND_LABELS: Record<DatastoreKind, string> = {
  postgres: 'PostgreSQL',
  redis: 'Redis',
}

/** One-line description of where a deploy's code came from. */
export function deploySourceLabel(source: DeploySource | null | undefined): string {
  if (!source) return EM_DASH
  switch (source.kind) {
    case 'git':
      return source.commit ? `${repoName(source.repo_url)}@${shortSha(source.commit)}` : `${repoName(source.repo_url)}@${source.branch}`
    case 'archive':
      return 'Uploaded archive'
    case 'image':
      return source.image
    case 'reuse':
      return source.from_deploy ? `Reused image from ${shortId(source.from_deploy)}` : `Reused ${source.image}`
  }
}
