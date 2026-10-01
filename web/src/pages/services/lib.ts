/**
 * Pure helpers of the services list (search, filter, sort, display strings).
 */

import {
  isLongRunning,
  sourceKind,
  type ServerInfo,
  type ServiceState,
  type ServiceType,
  type ServiceView,
} from '@/lib/api/types'
import { defaultDomain } from '@/lib/domains'
import { displayUrl, relativeTime, repoName, RUNTIME_LABELS, SERVICE_STATE_LABELS, SERVICE_TYPE_LABELS } from '@/lib/format'

export type ServicesSort = 'name' | 'last_deploy'
export type ServicesView = 'grid' | 'list'

/** Short uppercase tag of a service type (card badges). */
export const SERVICE_TYPE_TAGS: Record<ServiceType, string> = {
  web_service: 'web',
  private_service: 'private',
  background_worker: 'worker',
  cron_job: 'cron',
  static_site: 'static',
}

/** States in the order the Status filter lists them. */
export const FILTER_STATES: readonly ServiceState[] = ['live', 'deploying', 'degraded', 'failed', 'suspended', 'not_deployed']

/** Where the code comes from, as one line: `repo@branch`, the image, or the upload hint. */
export function sourceLine(s: ServiceView): string {
  switch (sourceKind(s)) {
    case 'git':
      return `${repoName(s.repo_url)}@${s.branch}`
    case 'image':
      return s.image ?? ''
    case 'upload':
      return 'Upload via ferry up'
  }
}

/** Timestamp of the latest deploy (ms), or null. */
export function lastDeployTime(s: ServiceView): number | null {
  const at = s.latest_deploy?.created_at
  if (!at) return null
  const t = new Date(at).getTime()
  return Number.isNaN(t) ? null : t
}

/** Case-insensitive search over name, type, runtime, source, URL and domains. */
export function matchesQuery(s: ServiceView, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  const haystack = [
    s.name,
    s.id,
    SERVICE_TYPE_LABELS[s.type],
    SERVICE_TYPE_TAGS[s.type],
    s.runtime,
    RUNTIME_LABELS[s.runtime],
    s.repo_url ?? '',
    s.branch,
    s.image ?? '',
    s.url ? displayUrl(s.url) : '',
    ...s.custom_domains,
    SERVICE_STATE_LABELS[s.state],
  ]
  return haystack.some((h) => h.toLowerCase().includes(q))
}

/** Sorted copy: by name, or by latest deploy (newest first, never-deployed last, then by name). */
export function sortServices(list: readonly ServiceView[], sort: ServicesSort): ServiceView[] {
  const byName = (a: ServiceView, b: ServiceView) => a.name.localeCompare(b.name)
  const copy = [...list]
  if (sort === 'name') return copy.sort(byName)
  return copy.sort((a, b) => {
    const ta = lastDeployTime(a)
    const tb = lastDeployTime(b)
    if (ta === tb) return byName(a, b)
    if (ta === null) return 1
    if (tb === null) return -1
    return tb - ta
  })
}

/** Number of services per state. */
export function countByState(list: readonly ServiceView[]): Record<ServiceState, number> {
  const counts: Record<ServiceState, number> = {
    live: 0,
    deploying: 0,
    degraded: 0,
    failed: 0,
    suspended: 0,
    not_deployed: 0,
  }
  for (const s of list) counts[s.state] += 1
  return counts
}

/** Parse the `?status=live,failed` search param (unknown values dropped). */
export function parseStatusParam(value: string | null): ServiceState[] {
  if (!value) return []
  const wanted = new Set(value.split(','))
  return FILTER_STATES.filter((s) => wanted.has(s))
}

/** Services that run long-lived containers right now (instances count toward "live instances"). */
export function runsInstances(s: ServiceView): boolean {
  return isLongRunning(s.type) && !s.suspended && s.live_deploy_id !== null && (s.state === 'live' || s.state === 'degraded')
}

/** Hosts that never get a public certificate (mirrors `ferry_core::config::is_local_host`). */
function isLocalHost(host: string): boolean {
  const h = host.toLowerCase()
  return h === 'localhost' || ['.localhost', '.local', '.internal', '.test'].some((s) => h.endsWith(s))
}

/**
 * URL a new public service named `name` would get (mirrors
 * `Config::url_for_host`; a hint only — the server decides).
 */
export function previewServiceUrl(
  name: string,
  info: Pick<ServerInfo, 'base_domain' | 'default_domain' | 'proxy_url' | 'tls_enabled' | 'dashboard_url'> | undefined,
): string | null {
  if (!info || !name) return null
  const host = `${name}.${defaultDomain(info)}`
  if (info.tls_enabled && !isLocalHost(host)) return `https://${host}`
  try {
    // The dashboard URL (when configured) carries the public port; else the proxy's.
    const ref = new URL(info.dashboard_url && !info.tls_enabled ? info.dashboard_url : info.proxy_url)
    const port = ref.port && ref.port !== '80' ? `:${ref.port}` : ''
    return `http://${host}${port}`
  } catch {
    return null
  }
}

/**
 * "3m ago" for a server timestamp. `now` comes from a 30s ticker, so a
 * timestamp newer than it (fresh deploy, clock skew) reads "just now"
 * instead of "in 5s".
 */
export function since(ts: string, now: number): string {
  const t = Date.parse(ts)
  return relativeTime(ts, Number.isNaN(t) ? now : Math.max(now, t))
}
