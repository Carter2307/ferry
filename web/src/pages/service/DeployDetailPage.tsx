import * as React from 'react'
import { Link, useParams } from 'react-router'
import { AlertTriangle, ArrowLeft, Ban, History, Undo2 } from 'lucide-react'

import { CopyButton } from '@/components/patterns/Copy'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { LogViewer } from '@/components/patterns/LogViewer'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { PageSection } from '@/components/patterns/Page'
import { DeployStatusBadge } from '@/components/patterns/StatusBadge'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { ApiError } from '@/lib/api/client'
import { streamPaths } from '@/lib/api/endpoints'
import { useDeploy } from '@/lib/api/queries'
import { isDeployActive, isDeployFailed, type Deploy, type ServiceView } from '@/lib/api/types'
import { useLogStream } from '@/lib/api/useLogStream'
import {
  dateTime,
  DEPLOY_TRIGGER_LABELS,
  deploySourceLabel,
  duration,
  relativeTime,
  shortDateTime,
  shortId,
  shortSha,
} from '@/lib/format'
import { cn } from '@/lib/utils'

import { servicePath, useServiceOutlet } from './context'
import { canRollbackTo, deployElapsedMs, useNow } from './runtime/utils'
import { useDeployActions } from './runtime/useDeployActions'

function BackLink({ name }: { name: string }) {
  return (
    <Link
      to={servicePath(name, '/deploys')}
      className="inline-flex w-fit items-center gap-1.5 rounded-sm text-[13px] text-foreground-lighter outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
    >
      <ArrowLeft className="size-3.5" aria-hidden="true" />
      All deploys
    </Link>
  )
}

function Meta({
  label,
  children,
  mono = false,
  className,
}: {
  label: string
  children: React.ReactNode
  mono?: boolean
  className?: string
}) {
  return (
    <div className={cn('flex min-w-0 flex-col gap-1 bg-surface-100 px-4 py-3.5 md:px-5', className)}>
      <MonoLabel as="dt">{label}</MonoLabel>
      <dd className={cn('min-w-0 truncate text-sm text-foreground', mono && 'font-mono text-[13px]')}>{children}</dd>
    </div>
  )
}

function Timestamp({ value }: { value: string | null }) {
  if (!value) return <span className="text-foreground-lighter">—</span>
  return (
    <time dateTime={value} title={`${dateTime(value)} (${value})`} className="flex min-w-0 flex-col">
      <span className="truncate">{shortDateTime(value)}</span>
      <span className="truncate text-[12.5px] text-foreground-lighter">{relativeTime(value)}</span>
    </time>
  )
}

function DeployHeader({
  service,
  deploy,
  onCancel,
  onRollback,
}: {
  service: ServiceView
  deploy: Deploy
  onCancel: () => void
  onRollback: () => void
}) {
  const active = isDeployActive(deploy.status)
  const now = useNow(1000, active)
  const elapsed = deployElapsedMs(deploy, now)
  const isCurrent = deploy.id === service.live_deploy_id
  const rollbackable = canRollbackTo(deploy, service.live_deploy_id)

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="flex min-w-0 flex-col gap-1.5">
          <div className="flex min-w-0 flex-wrap items-center gap-2.5">
            <h2 className="text-lg font-medium text-foreground md:text-xl">
              Deploy <span className="font-mono text-[0.92em]">{shortId(deploy.id)}</span>
            </h2>
            <DeployStatusBadge status={deploy.status} />
            {isCurrent && (
              <Badge variant="success" font="mono">
                Current
              </Badge>
            )}
            <CopyButton value={deploy.id} what="deploy ID" />
          </div>
          <p className="min-w-0 text-sm break-words text-foreground-light">
            {deploy.commit_message ? (
              <>
                <span className="font-mono text-foreground">{shortSha(deploy.commit_sha)}</span> {deploy.commit_message}
              </>
            ) : (
              <>
                {DEPLOY_TRIGGER_LABELS[deploy.trigger]} · {deploySourceLabel(deploy.source)}
              </>
            )}
          </p>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          {rollbackable && (
            <Button icon={<Undo2 />} onClick={onRollback} disabled={service.suspended}>
              Roll back to this deploy
            </Button>
          )}
          {active && (
            <Button variant="danger" icon={<Ban />} onClick={onCancel}>
              Cancel deploy
            </Button>
          )}
        </div>
      </div>

      <dl className="grid grid-cols-1 gap-px overflow-hidden rounded-lg border bg-border shadow-card sm:grid-cols-2 lg:grid-cols-4">
        <Meta label="Trigger">{DEPLOY_TRIGGER_LABELS[deploy.trigger]}</Meta>
        <Meta label="Source" mono>
          <span title={deploySourceLabel(deploy.source)}>{deploySourceLabel(deploy.source)}</span>
        </Meta>
        <Meta label="Commit" mono>
          {deploy.commit_sha ? (
            <span title={deploy.commit_sha}>{shortSha(deploy.commit_sha)}</span>
          ) : (
            <span className="text-foreground-lighter">—</span>
          )}
        </Meta>
        <Meta label="Image" mono>
          {deploy.image ? (
            <span title={deploy.image}>{deploy.image}</span>
          ) : (
            <span className="text-foreground-lighter">{active ? 'not built yet' : 'none'}</span>
          )}
        </Meta>
        <Meta label="Created">
          <Timestamp value={deploy.created_at} />
        </Meta>
        <Meta label="Started">
          <Timestamp value={deploy.started_at} />
        </Meta>
        <Meta label="Finished">
          {active ? (
            <span className="text-foreground-lighter">In progress…</span>
          ) : (
            <Timestamp value={deploy.finished_at} />
          )}
        </Meta>
        <Meta label="Duration" mono>
          <span className="tabular">{elapsed === null ? '—' : duration(elapsed)}</span>
          {deploy.port ? <span className="ml-2 text-foreground-lighter">· port {deploy.port}</span> : null}
        </Meta>
      </dl>

      {deploy.error && (
        <Callout
          tone={isDeployFailed(deploy.status) ? 'destructive' : 'neutral'}
          icon={<AlertTriangle />}
          title={isDeployFailed(deploy.status) ? 'Why it failed' : 'Note'}
        >
          <span className="font-mono text-[12.5px] break-words whitespace-pre-wrap">{deploy.error}</span>
        </Callout>
      )}
    </div>
  )
}

function HeaderSkeleton() {
  return (
    <div className="flex flex-col gap-5" aria-busy="true" aria-label="Loading deploy">
      <Skeleton className="h-7 w-64" />
      <Skeleton className="h-4 w-80" />
      <Skeleton className="h-[146px] w-full rounded-lg" />
    </div>
  )
}

function DeployLogs({ deploy }: { deploy: Deploy }) {
  const stream = useLogStream(streamPaths.deployLogs(deploy.id), { follow: true, maxLines: 20000 })
  const emptyText =
    deploy.status === 'queued'
      ? 'Waiting in the queue — the log starts when the build begins.'
      : stream.status === 'ended'
        ? 'This deploy produced no log lines.'
        : 'No log lines yet.'
  return (
    <PageSection title="Build & deploy log" description="Streams live until the deploy finishes.">
      <LogViewer
        lines={stream.lines}
        status={stream.status}
        error={stream.error}
        onRestart={stream.restart}
        showInstance={false}
        downloadName={`deploy-${deploy.id}`}
        emptyText={emptyText}
        label={`Log of deploy ${deploy.id}`}
      />
    </PageSection>
  )
}

/** `/services/:name/deploys/:deployId` — deploy header + streaming build/deploy log. */
export function DeployDetailPage() {
  const { service, name } = useServiceOutlet()
  const { deployId = '' } = useParams()
  const query = useDeploy(deployId)
  const actions = useDeployActions(service)
  const deploy = query.data && query.data.service_id === service.id ? query.data : undefined
  const notFound =
    (query.error instanceof ApiError && query.error.isNotFound) ||
    (query.data !== undefined && query.data.service_id !== service.id)

  return (
    <div className="flex flex-col gap-6">
      <BackLink name={name} />
      {notFound ? (
        <EmptyState
          size="lg"
          icon={<History />}
          title="Deploy not found"
          description={
            <>
              <span className="font-mono">{deployId}</span> is not a deploy of {name}.
            </>
          }
          actions={
            <Button asChild size="tiny">
              <Link to={servicePath(name, '/deploys')}>All deploys</Link>
            </Button>
          }
        />
      ) : query.error && !deploy ? (
        <ErrorState
          error={query.error}
          title="Could not load the deploy"
          onRetry={() => void query.refetch()}
          retrying={query.isRefetching}
        />
      ) : !deploy ? (
        <HeaderSkeleton />
      ) : (
        <>
          <DeployHeader
            service={service}
            deploy={deploy}
            onCancel={() => actions.requestCancel(deploy)}
            onRollback={() => actions.requestRollback(deploy)}
          />
          <DeployLogs key={deploy.id} deploy={deploy} />
        </>
      )}
      {actions.dialogs}
    </div>
  )
}
