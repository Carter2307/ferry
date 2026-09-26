import { Link, useNavigate } from 'react-router'
import { Ban, Copy, Ellipsis, ScrollText, Undo2 } from 'lucide-react'
import { toast } from 'sonner'

import { copyText } from '@/hooks/useCopy'
import { DeployStatusBadge } from '@/components/patterns/StatusBadge'
import { TableErrorRow, TableMessageRow, TableSkeletonRows } from '@/components/patterns/DataTable'
import { rowLinkProps } from '@/components/patterns/table-utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { isDeployActive, type Deploy, type ServiceView } from '@/lib/api/types'
import {
  dateTime,
  DEPLOY_TRIGGER_LABELS,
  deploySourceLabel,
  duration,
  relativeTime,
  shortId,
  shortSha,
} from '@/lib/format'
import { cn } from '@/lib/utils'

import { servicePath } from '../context'
import { canRollbackTo, deployElapsedMs, useNow } from './utils'

interface DeploysTableProps {
  service: ServiceView
  deploys: Deploy[] | undefined
  loading: boolean
  error: unknown
  onCancel: (deploy: Deploy) => void
  onRollback: (deploy: Deploy) => void
}

const COLUMNS = 7

function deployPath(service: ServiceView, d: Deploy) {
  return servicePath(service.name, `/deploys/${encodeURIComponent(d.id)}`)
}

function RowMenu({
  service,
  deploy,
  onCancel,
  onRollback,
}: {
  service: ServiceView
  deploy: Deploy
  onCancel: (d: Deploy) => void
  onRollback: (d: Deploy) => void
}) {
  const navigate = useNavigate()
  const active = isDeployActive(deploy.status)
  const rollbackable = canRollbackTo(deploy, service.live_deploy_id)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          size="icon-tiny"
          variant="ghost"
          icon={<Ellipsis />}
          aria-label={`Actions for deploy ${shortId(deploy.id)}`}
        />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuItem onSelect={() => void navigate(deployPath(service, deploy))}>
          <ScrollText /> View logs
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={() =>
            void copyText(deploy.id).then((ok) =>
              ok
                ? toast.success('Deploy ID copied', { description: deploy.id })
                : toast.error('Could not copy to the clipboard'),
            )
          }
        >
          <Copy /> Copy deploy ID
        </DropdownMenuItem>
        {(rollbackable || active) && <DropdownMenuSeparator />}
        {rollbackable && (
          <DropdownMenuItem onSelect={() => onRollback(deploy)} disabled={service.suspended}>
            <Undo2 /> Roll back to this deploy
          </DropdownMenuItem>
        )}
        {active && (
          <DropdownMenuItem variant="destructive" onSelect={() => onCancel(deploy)}>
            <Ban /> Cancel deploy
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** Deploy history table (Studio bordered table, mono uppercase headers). */
export function DeploysTable({ service, deploys, loading, error, onCancel, onRollback }: DeploysTableProps) {
  const navigate = useNavigate()
  const anyActive = deploys?.some((d) => isDeployActive(d.status)) ?? false
  // 1s while something builds (durations), 15s otherwise (relative times)
  const now = useNow(anyActive ? 1000 : 15000)

  return (
    <Table aria-label={`Deploys of ${service.name}`}>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead className="hidden w-[1%] sm:table-cell">Status</TableHead>
          <TableHead>Deploy</TableHead>
          <TableHead className="hidden sm:table-cell">Source</TableHead>
          <TableHead className="hidden xl:table-cell">Image</TableHead>
          <TableHead className="hidden text-right lg:table-cell">Duration</TableHead>
          <TableHead className="hidden text-right md:table-cell">Created</TableHead>
          <TableHead className="w-[1%]">
            <span className="sr-only">Actions</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {loading && !deploys ? (
          <TableSkeletonRows columns={COLUMNS} rows={5} />
        ) : error && !deploys ? (
          <TableErrorRow colSpan={COLUMNS} error={error} />
        ) : deploys && deploys.length === 0 ? (
          <TableMessageRow colSpan={COLUMNS}>No deploys yet. Trigger one with Manual deploy.</TableMessageRow>
        ) : (
          deploys?.map((d) => {
            const isLive = d.id === service.live_deploy_id
            const elapsed = deployElapsedMs(d, now)
            return (
              <TableRow
                key={d.id}
                {...rowLinkProps(() => void navigate(deployPath(service, d)))}
                className={cn('cursor-pointer', isLive && 'bg-success-soft/40')}
              >
                <TableCell className="hidden sm:table-cell">
                  <DeployStatusBadge status={d.status} />
                </TableCell>
                <TableCell className="pr-2 sm:pr-4">
                  <div className="flex flex-col gap-0.5">
                    <DeployStatusBadge status={d.status} className="mb-1 sm:hidden" />
                    <Link
                      to={deployPath(service, d)}
                      className="w-fit rounded-sm font-mono text-[13px] text-foreground underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      {shortId(d.id)}
                    </Link>
                    <span className="flex items-center gap-1.5 text-[12px] text-foreground-lighter">
                      {DEPLOY_TRIGGER_LABELS[d.trigger]}
                      {isLive && (
                        <Badge variant="success" font="mono" className="h-4 px-1.5 text-[9.5px]">
                          Current
                        </Badge>
                      )}
                    </span>
                    {/* narrow screens: commit + age under the id (their columns are hidden) */}
                    <span className="flex min-w-0 items-center gap-1.5 text-[12px] text-foreground-lighter md:hidden">
                      {d.commit_sha && (
                        <span className="font-mono text-foreground-light sm:hidden">{shortSha(d.commit_sha)} ·</span>
                      )}
                      <time dateTime={d.created_at}>
                        {relativeTime(d.created_at, Math.max(now, Date.parse(d.created_at)))}
                      </time>
                    </span>
                  </div>
                </TableCell>
                <TableCell className="hidden max-w-[340px] whitespace-normal sm:table-cell">
                  {d.commit_sha ? (
                    <div className="flex min-w-0 flex-col gap-0.5">
                      <span className="font-mono text-[13px] text-foreground">{shortSha(d.commit_sha)}</span>
                      <span
                        className="truncate text-[12px] text-foreground-light"
                        title={d.commit_message ?? undefined}
                      >
                        {d.commit_message ?? '—'}
                      </span>
                    </div>
                  ) : (
                    <span
                      className="block truncate text-[13px] text-foreground-light"
                      title={deploySourceLabel(d.source)}
                    >
                      {deploySourceLabel(d.source)}
                    </span>
                  )}
                </TableCell>
                <TableCell className="hidden max-w-[240px] xl:table-cell">
                  <span
                    className="block truncate font-mono text-[12px] text-foreground-light"
                    title={d.image ?? undefined}
                  >
                    {d.image ?? '—'}
                  </span>
                </TableCell>
                <TableCell className="hidden text-right font-mono text-[12.5px] text-foreground-light tabular lg:table-cell">
                  {elapsed === null ? '—' : duration(elapsed)}
                </TableCell>
                <TableCell className="hidden text-right text-[13px] text-foreground-light md:table-cell">
                  <time dateTime={d.created_at} title={dateTime(d.created_at)}>
                    {relativeTime(d.created_at, Math.max(now, Date.parse(d.created_at)))}
                  </time>
                </TableCell>
                <TableCell className="w-[1%] px-2 text-right">
                  <RowMenu service={service} deploy={d} onCancel={onCancel} onRollback={onRollback} />
                </TableCell>
              </TableRow>
            )
          })
        )}
      </TableBody>
    </Table>
  )
}
