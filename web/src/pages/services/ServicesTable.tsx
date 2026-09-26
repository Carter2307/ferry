import { Link, useNavigate } from 'react-router'

import { ServiceTypeIcon } from '@/components/patterns/icons'
import { ServiceStatePill } from '@/components/patterns/StatusBadge'
import { rowLinkProps } from '@/components/patterns/table-utils'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { isLongRunning, sourceKind, type ServiceView } from '@/lib/api/types'
import { displayUrl, SERVICE_TYPE_LABELS } from '@/lib/format'
import { cn } from '@/lib/utils'
import { servicePath } from '@/pages/service/context'

import { LastDeploy } from './LastDeploy'
import { SERVICE_TYPE_TAGS, sourceLine } from './lib'
import { ServiceMenu } from './ServiceMenu'

/**
 * Column visibility by table width (container queries), shared by header, rows
 * and skeleton. Budget ≈ service 220 + status 110 + actions 48 (always shown),
 * last deploy 150, type 80, instances 100, source 200: each threshold leaves
 * room for every visible column so the table never scrolls sideways next to
 * the server panel (~770px at 1280).
 */
const COL = {
  lastDeploy: 'hidden @lg:table-cell',
  type: 'hidden @2xl:table-cell',
  instances: 'hidden @4xl:table-cell',
  source: 'hidden @5xl:table-cell',
} as const

function SkeletonRows() {
  return Array.from({ length: 5 }, (_, i) => (
    <TableRow key={i} className="hover:bg-transparent" aria-hidden="true">
      <TableCell>
        <div className="flex items-center gap-3">
          <Skeleton className="size-4 rounded-sm" />
          <div className="flex flex-col gap-1.5">
            <Skeleton className="h-3.5 w-24" />
            <Skeleton className="h-3 w-32" />
          </div>
        </div>
      </TableCell>
      <TableCell>
        <Skeleton className="h-5 w-14 rounded-full" />
      </TableCell>
      <TableCell className={COL.type}>
        <Skeleton className="h-5 w-12 rounded-sm" />
      </TableCell>
      <TableCell className={COL.source}>
        <Skeleton className="h-3.5 w-28" />
      </TableCell>
      <TableCell className={COL.instances}>
        <Skeleton className="h-3.5 w-6" />
      </TableCell>
      <TableCell className={COL.lastDeploy}>
        <Skeleton className="h-3.5 w-16" />
      </TableCell>
      <TableCell />
    </TableRow>
  ))
}

/** List view of the services (Studio table: mono headers, clickable rows). */
export function ServicesTable({ services, loading, now }: { services: ServiceView[]; loading: boolean; now: number }) {
  const navigate = useNavigate()
  return (
    <div className="@container">
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>Service</TableHead>
            <TableHead>Status</TableHead>
            <TableHead className={COL.type}>Type</TableHead>
            <TableHead className={COL.source}>Source</TableHead>
            <TableHead className={COL.instances}>Instances</TableHead>
            <TableHead className={COL.lastDeploy}>Last deploy</TableHead>
            <TableHead className="w-12">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {loading ? (
            <SkeletonRows />
          ) : (
            services.map((s) => {
              const latest = s.latest_deploy
              const kind = sourceKind(s)
              return (
                <TableRow key={s.id} {...rowLinkProps(() => void navigate(servicePath(s.name)))}>
                  <TableCell className="max-w-[220px]">
                    <div className="flex min-w-0 items-center gap-3">
                      <ServiceTypeIcon type={s.type} className="size-4 shrink-0 text-foreground-lighter" />
                      <div className="flex min-w-0 flex-col">
                        <Link
                          to={servicePath(s.name)}
                          className="truncate font-medium text-foreground outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                        >
                          {s.name}
                        </Link>
                        <span className="truncate text-[12.5px] text-foreground-lighter">
                          {s.url ? displayUrl(s.url) : s.type === 'cron_job' ? `cron ${s.schedule ?? ''}` : s.internal_host}
                        </span>
                      </div>
                    </div>
                  </TableCell>
                  <TableCell>
                    <ServiceStatePill state={s.state} />
                  </TableCell>
                  <TableCell className={COL.type}>
                    <Badge font="mono" shape="square" title={SERVICE_TYPE_LABELS[s.type]}>
                      {SERVICE_TYPE_TAGS[s.type]}
                    </Badge>
                  </TableCell>
                  <TableCell className={cn(COL.source, 'max-w-[220px]')}>
                    <span
                      className={cn('block truncate text-[13px] text-foreground-light', kind !== 'upload' && 'font-mono text-[12.5px]')}
                      title={sourceLine(s)}
                    >
                      {sourceLine(s)}
                    </span>
                  </TableCell>
                  <TableCell className={cn(COL.instances, 'font-mono text-[13px] text-foreground-light tabular')}>
                    {isLongRunning(s.type) ? `×${s.instances}` : '—'}
                  </TableCell>
                  <TableCell className={cn(COL.lastDeploy, 'text-[13px] text-foreground-light')}>
                    {latest ? (
                      <LastDeploy deploy={latest} now={now} className="text-[13px] text-foreground-light" />
                    ) : (
                      <span className="text-foreground-lighter">Never</span>
                    )}
                  </TableCell>
                  <TableCell className="pr-3 text-right">
                    <ServiceMenu service={s} />
                  </TableCell>
                </TableRow>
              )
            })
          )}
        </TableBody>
      </Table>
    </div>
  )
}
