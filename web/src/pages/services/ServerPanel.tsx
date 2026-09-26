import * as React from 'react'
import { Link } from 'react-router'
import { useQueries } from '@tanstack/react-query'
import { Boxes, Braces, Container, Database, Globe, Lock, Server, Tag } from 'lucide-react'

import { MonoLabel } from '@/components/patterns/MonoLabel'
import { StatusDot } from '@/components/patterns/StatusBadge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { endpoints } from '@/lib/api/endpoints'
import { useLive, type LiveMode } from '@/lib/api/events'
import { keys } from '@/lib/api/keys'
import { useDatastores, useEnvGroups, useServerInfo } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'
import { displayUrl } from '@/lib/format'
import { cn } from '@/lib/utils'

import { runsInstances } from './lib'

const LIVE_LABELS: Record<LiveMode, string> = {
  live: 'Live updates',
  polling: 'Refreshing every 5s',
  connecting: 'Connecting…',
  reconnecting: 'Reconnecting…',
  off: 'Offline',
}

/** 16px circular progress (Studio usage rows). */
function UsageRing({ value, max }: { value: number; max: number }) {
  const r = 6
  const c = 2 * Math.PI * r
  const pct = max > 0 ? Math.min(1, value / max) : 0
  const tone = max === 0 ? 'var(--border-stronger)' : pct >= 1 ? 'var(--brand)' : 'var(--warning)'
  return (
    <svg viewBox="0 0 16 16" className="size-4 shrink-0 -rotate-90" aria-hidden="true">
      <circle cx="8" cy="8" r={r} fill="none" stroke="var(--border-strong)" strokeWidth="2" />
      <circle
        cx="8"
        cy="8"
        r={r}
        fill="none"
        stroke={tone}
        strokeWidth="2"
        strokeLinecap="round"
        strokeDasharray={`${c * pct} ${c}`}
        className="transition-[stroke-dasharray] duration-500"
      />
    </svg>
  )
}

interface RowProps {
  icon: React.ReactNode
  label: string
  value: React.ReactNode
  hint?: React.ReactNode
  to?: string
  loading?: boolean
}

function Row({ icon, label, value, hint, to, loading }: RowProps) {
  const body = (
    <>
      <span className="flex size-4 shrink-0 items-center justify-center text-foreground-lighter [&_svg]:size-4">{icon}</span>
      <span className="min-w-0 flex-1 truncate text-[13px] text-foreground-light">{label}</span>
      {loading ? (
        <Skeleton className="h-4 w-14" />
      ) : (
        <span className="flex min-w-0 items-baseline gap-1.5 text-right text-[13px] text-foreground tabular">
          {hint && <span className="hidden text-[12px] text-foreground-lighter sm:inline lg:hidden 2xl:inline">{hint}</span>}
          <span className="min-w-0 truncate">{value}</span>
        </span>
      )}
    </>
  )
  const cls = 'flex min-h-11 items-center gap-3 border-t px-4 py-2.5'
  return to ? (
    <li>
      <Link
        to={to}
        className={cn(cls, 'outline-none transition-colors hover:bg-surface-200 focus-visible:bg-surface-200 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset')}
      >
        {body}
      </Link>
    </li>
  ) : (
    <li className={cls}>{body}</li>
  )
}

/**
 * Running containers vs desired instances. Live services run all their
 * instances by definition; degraded ones are asked for their status.
 */
function useInstanceCounts(services: ServiceView[] | undefined) {
  const running = services?.filter(runsInstances) ?? []
  const degraded = running.filter((s) => s.state === 'degraded')
  const statuses = useQueries({
    queries: degraded.map((s) => ({
      queryKey: keys.serviceStatus(s.name),
      queryFn: ({ signal }: { signal: AbortSignal }) => endpoints.serviceStatus(s.name, signal),
      refetchInterval: 10_000,
    })),
  })
  const desired = running.reduce((n, s) => n + s.instances, 0)
  let up = 0
  let pending = false
  running.forEach((s) => {
    if (s.state === 'live') {
      up += s.instances
      return
    }
    const i = degraded.indexOf(s)
    const status = statuses[i]?.data
    if (!status) {
      pending = true
      return
    }
    up += status.instances.filter((x) => x.state === 'running' && x.deploy_id === s.live_deploy_id).length
  })
  return { up, desired, pending }
}

/** Right column of the services page: server facts and resource counts, like Studio's usage list. */
export function ServerPanel({ services, loading }: { services: ServiceView[] | undefined; loading: boolean }) {
  const info = useServerInfo()
  const datastores = useDatastores()
  const groups = useEnvGroups()
  const live = useLive((s) => s.mode)
  const counts = useInstanceCounts(services)
  const liveServices = services?.filter((s) => s.state === 'live').length ?? 0
  const i = info.data

  return (
    <aside aria-labelledby="server-panel-title" className="flex flex-col overflow-hidden rounded-lg border bg-surface-100 shadow-card">
      <div className="flex items-start justify-between gap-3 px-4 py-4">
        <div className="flex min-w-0 flex-col gap-0.5">
          <h2 id="server-panel-title" className="text-sm font-medium text-foreground">
            Ferry server
          </h2>
          {info.isLoading ? (
            <Skeleton className="mt-1 h-3.5 w-32" />
          ) : (
            <p className="truncate text-[13px] text-foreground-light">
              {i ? `${i.base_domain} · ${i.tls_enabled ? 'HTTPS' : 'HTTP'}` : 'Server info unavailable'}
            </p>
          )}
        </div>
        <Button asChild size="tiny" icon={<Server />}>
          <Link to="/server">Details</Link>
        </Button>
      </div>
      <ul className="flex flex-col">
        <Row icon={<Tag />} label="Version" loading={info.isLoading} value={i ? <span className="font-mono">v{i.version}</span> : '—'} />
        <Row
          icon={<Container />}
          label="Docker"
          loading={info.isLoading}
          value={
            i?.docker_version ? (
              <span className="font-mono">{i.docker_version}</span>
            ) : (
              <span className="text-warning">Unavailable</span>
            )
          }
        />
        <Row
          icon={i?.tls_enabled ? <Lock /> : <Globe />}
          label="Proxy"
          loading={info.isLoading}
          value={i ? <span className="font-mono text-[12.5px]">{displayUrl(i.proxy_url)}</span> : '—'}
        />
        <Row
          icon={<Boxes />}
          label="Services"
          loading={loading}
          hint={services && services.length > 0 ? `${liveServices} live` : undefined}
          value={services?.length ?? '—'}
        />
        <Row
          icon={<Database />}
          label="Datastores"
          to="/datastores"
          loading={datastores.isLoading}
          value={datastores.data?.length ?? '—'}
        />
        <Row icon={<Braces />} label="Env groups" to="/env-groups" loading={groups.isLoading} value={groups.data?.length ?? '—'} />
        <Row
          icon={<UsageRing value={counts.up} max={counts.desired} />}
          label="Live instances"
          loading={loading || counts.pending}
          value={
            <>
              {counts.up} <span className="text-foreground-lighter">/ {counts.desired}</span>
            </>
          }
        />
      </ul>
      <div className="flex items-center gap-2 border-t bg-surface-75 px-4 py-2.5 dark:bg-transparent">
        <StatusDot tone={live === 'live' ? 'success' : live === 'off' ? 'neutral' : 'warning'} pulse={live === 'live'} />
        <MonoLabel className="text-[11px]">
          {LIVE_LABELS[live]}
        </MonoLabel>
      </div>
    </aside>
  )
}
