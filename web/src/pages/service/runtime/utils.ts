import * as React from 'react'

import { isDeployActive, sourceKind, type Deploy, type InstanceStatus, type ServiceView } from '@/lib/api/types'
import { repoName } from '@/lib/format'

/**
 * Current time, refreshed every `intervalMs` while `enabled` (live durations,
 * relative times). Also refreshes right away when the cadence changes.
 */
export function useNow(intervalMs = 1000, enabled = true): number {
  const [now, setNow] = React.useState(() => Date.now())
  React.useEffect(() => {
    if (!enabled) return
    const tick = () => setNow(Date.now())
    const id = window.setInterval(tick, intervalMs)
    const raf = window.requestAnimationFrame(tick)
    return () => {
      window.clearInterval(id)
      window.cancelAnimationFrame(raf)
    }
  }, [intervalMs, enabled])
  return now
}

/**
 * Rollback targets, as the engine allows them: deploys that served traffic
 * (`live` or `deactivated`), still have an image, and are not the live one.
 */
export function canRollbackTo(deploy: Deploy, liveDeployId: string | null): boolean {
  return (
    (deploy.status === 'live' || deploy.status === 'deactivated') && Boolean(deploy.image) && deploy.id !== liveDeployId
  )
}

/** Build + deploy time in ms (still counting while active); null before it started. */
export function deployElapsedMs(deploy: Deploy, now: number): number | null {
  if (!deploy.started_at) return null
  const start = new Date(deploy.started_at).getTime()
  const end = deploy.finished_at ? new Date(deploy.finished_at).getTime() : isDeployActive(deploy.status) ? now : null
  if (end === null || Number.isNaN(start)) return null
  return Math.max(0, end - start)
}

/** Short instance id as used in runtime log lines (last 6 chars of the container name). */
export function instanceShortId(instance: Pick<InstanceStatus, 'name'>): string {
  return instance.name.slice(-6)
}

/** `host:port` (or just the host for services without a port). */
export function privateAddress(service: Pick<ServiceView, 'internal_host' | 'internal_port'>): string {
  return service.internal_port ? `${service.internal_host}:${service.internal_port}` : service.internal_host
}

/** One-line description of the service's code source. */
export function sourceSummary(service: ServiceView): {
  kind: 'git' | 'image' | 'upload'
  label: string
  detail: string
} {
  const kind = sourceKind(service)
  if (kind === 'git') {
    return { kind, label: `${repoName(service.repo_url)}@${service.branch}`, detail: service.repo_url ?? '' }
  }
  if (kind === 'image') return { kind, label: service.image ?? '', detail: 'Prebuilt image' }
  return { kind, label: 'Uploaded archive', detail: `ferry up ${service.name}` }
}

/** Label of the manual deploy action for this source. */
export function deployActionLabel(service: ServiceView): string {
  const kind = sourceKind(service)
  if (kind === 'git') return `Deploy latest commit of ${service.branch}`
  if (kind === 'image') return 'Pull image & deploy'
  return 'Redeploy last upload'
}
