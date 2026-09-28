import * as React from 'react'
import { Link } from 'react-router'
import { Boxes, CalendarClock, MemoryStick, Pause, Play, Rocket } from 'lucide-react'
import { toast } from 'sonner'

import { CopyButton } from '@/components/patterns/Copy'
import { EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { LegendDot, MetricCard, UsageBar } from '@/components/patterns/MetricCard'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { PageSection } from '@/components/patterns/Page'
import { StatePill, StatusDot } from '@/components/patterns/StatusBadge'
import type { StatusTone } from '@/components/patterns/status-tones'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Hint } from '@/components/ui/tooltip'
import { errorMessage } from '@/lib/api/client'
import { useResumeService } from '@/lib/api/queries'
import type { InstanceStatus, RuntimeStatus, ServiceView } from '@/lib/api/types'
import { bytes, dateTime, logTime, percent, relativeTime } from '@/lib/format'
import { formatCpus } from '@/lib/resources'
import { cn } from '@/lib/utils'

import { servicePath } from '../context'
import { instanceShortId } from './utils'

/** Samples kept per instance for the CPU mini chart. */
const HISTORY = 36

interface Sample {
  t: number
  cpu: number | null
}

type History = Record<string, Sample[]>

/**
 * CPU history per container, appended each time the status query returns
 * fresh data (render-time update keyed by `updatedAt`, no effect needed).
 */
function useCpuHistory(status: RuntimeStatus | undefined, updatedAt: number): History {
  const [state, setState] = React.useState<{ at: number; history: History }>({ at: 0, history: {} })
  if (status && updatedAt !== state.at) {
    const next: History = {}
    for (const inst of status.instances) {
      const prev = state.history[inst.container_id] ?? []
      next[inst.container_id] = [...prev, { t: updatedAt, cpu: inst.cpu_percent }].slice(-HISTORY)
    }
    setState({ at: updatedAt, history: next })
  }
  return state.history
}

const INSTANCE_STATE_TONE: Record<string, StatusTone> = {
  running: 'success',
  created: 'neutral',
  restarting: 'warning',
  paused: 'neutral',
  exited: 'destructive',
  dead: 'destructive',
  removing: 'neutral',
}

/** Thin bar chart of recent CPU samples (Studio metric-card look). */
function CpuBars({ samples, label }: { samples: Sample[]; label: string }) {
  const values = samples.map((s) => s.cpu ?? 0)
  const peak = Math.max(0, ...values)
  const scale = Math.max(5, peak * 1.25)
  const slots = Array.from({ length: HISTORY }, (_, i) => samples[i - (HISTORY - samples.length)] ?? null)
  const first = samples[0]
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <LegendDot color="var(--brand)">CPU usage</LegendDot>
        <span className="font-mono text-[10.5px] tracking-[0.04em] text-foreground-lighter tabular">
          peak {percent(peak)}
        </span>
      </div>
      <div
        role="img"
        aria-label={`${label}: last ${samples.length} samples, peak ${percent(peak)}`}
        className="flex h-12 items-end gap-[3px] border-b border-dashed border-border-strong pb-px"
      >
        {slots.map((s, i) => {
          const v = s?.cpu ?? 0
          const h = s ? Math.max(3, Math.round((v / scale) * 46)) : 3
          return (
            <span
              key={i}
              className={cn(
                'min-w-0 flex-1 rounded-[1.5px]',
                s
                  ? v >= 90
                    ? 'bg-destructive-solid'
                    : v >= 75
                      ? 'bg-chart-2'
                      : 'bg-brand/85'
                  : 'bg-foreground/[0.06]',
              )}
              style={{ height: h }}
            />
          )
        })}
      </div>
      <div className="flex justify-between font-mono text-[10.5px] tracking-[0.04em] text-foreground-lighter tabular">
        <span>{first ? logTime(new Date(first.t).toISOString(), false) : '—'}</span>
        <span>now</span>
      </div>
    </div>
  )
}

/**
 * `OOM KILLED · EXIT 137` pill with an explanation tooltip, for instances
 * whose last exit was the kernel's out-of-memory killer.
 */
export function OomBadge({ instance }: { instance: Pick<InstanceStatus, 'exit_code' | 'memory_limit_bytes'> }) {
  const limit = instance.memory_limit_bytes
  return (
    <Hint
      label={
        <>
          The kernel stopped this container because it used more memory than its limit
          {limit ? ` (${bytes(limit)})` : ''}. Ferry starts it again; if it keeps happening, raise the memory limit in
          Settings → Resources.
        </>
      }
    >
      <button
        type="button"
        className="w-fit cursor-help rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <StatePill
          tone="destructive"
          dot={false}
          label={
            <>
              <MemoryStick aria-hidden="true" className="size-3" />
              OOM killed{instance.exit_code !== null ? ` · exit ${instance.exit_code}` : ''}
            </>
          }
        />
      </button>
    </Hint>
  )
}

/** `limit 512 MiB` / `no limit` under a metric. */
function LimitLine({ limit, title }: { limit: string | null; title?: string }) {
  return (
    <span className="truncate text-[12px] text-foreground-lighter" title={title}>
      {limit ? `limit ${limit}` : 'no limit'}
    </span>
  )
}

export function InstanceCard({
  instance,
  samples,
  settingsPath,
}: {
  instance: InstanceStatus
  samples: Sample[]
  /** Service settings page (the OOM notice links to its Resources section). */
  settingsPath?: string
}) {
  const short = instanceShortId(instance)
  const memoryLimit = instance.memory_limit_bytes
  const memPct = instance.memory_bytes !== null && memoryLimit ? (instance.memory_bytes / memoryLimit) * 100 : null
  const tone = INSTANCE_STATE_TONE[instance.state] ?? 'neutral'
  const running = instance.state === 'running'
  // An exit code says why a stopped container stopped (137 alone is not proof of OOM: `oom_killed` is).
  const exitCode = !running && !instance.oom_killed && instance.exit_code !== null ? instance.exit_code : null

  return (
    <MetricCard
      label={
        <span className="flex items-center gap-1.5">
          Instance <span className="text-foreground">{short}</span>
        </span>
      }
      aside={
        <StatePill
          tone={tone}
          label={exitCode !== null ? `${instance.state} · ${exitCode}` : instance.state}
          title={exitCode !== null ? `Exit code ${exitCode}` : undefined}
          pulse={instance.state === 'restarting'}
        />
      }
      className="gap-4"
    >
      {instance.oom_killed && (
        <div className="-mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12.5px] text-foreground-light">
          <OomBadge instance={instance} />
          <span>Ran out of memory{memoryLimit ? ` (limit ${bytes(memoryLimit)})` : ''}.</span>
          {settingsPath && (
            <Link
              to={`${settingsPath}#resources`}
              className="rounded-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
            >
              Raise the limit
            </Link>
          )}
        </div>
      )}
      <div className="grid grid-cols-2 gap-4">
        <div className="flex min-w-0 flex-col gap-1">
          <MonoLabel>CPU</MonoLabel>
          <span className="text-xl text-foreground tabular md:text-[22px]">
            {running ? percent(instance.cpu_percent) : '—'}
          </span>
          <LimitLine
            limit={instance.cpu_limit ? formatCpus(instance.cpu_limit) : null}
            title={
              instance.cpu_limit
                ? `100% = one full core, so this instance tops out at ${percent(instance.cpu_limit * 100, 0)}.`
                : '100% = one full core.'
            }
          />
        </div>
        <div className="flex min-w-0 flex-col gap-1">
          <MonoLabel>Memory</MonoLabel>
          <span className="truncate text-xl text-foreground tabular md:text-[22px]">
            {running ? bytes(instance.memory_bytes) : '—'}
          </span>
          <LimitLine limit={memoryLimit ? bytes(memoryLimit) : null} />
        </div>
      </div>
      <CpuBars samples={samples} label={`CPU of instance ${short}`} />
      <div className="flex flex-col gap-1.5">
        <div className="flex items-center justify-between gap-2">
          <LegendDot color="var(--brand)">Memory used</LegendDot>
          <span className="font-mono text-[10.5px] tracking-[0.04em] text-foreground-lighter tabular">
            {!running || instance.memory_bytes === null
              ? 'no data'
              : memPct !== null && memoryLimit
                ? `${percent(memPct)} of ${bytes(memoryLimit)}`
                : 'no limit'}
          </span>
        </div>
        <UsageBar value={running ? memPct : 0} label={`Memory of instance ${short}`} />
      </div>
      <dl className="-mx-5 -mb-5 grid grid-cols-3 border-t text-[13px]">
        <div className="flex min-w-0 flex-col gap-0.5 py-3 pr-3 pl-5">
          <dt className="mono-label">Restarts</dt>
          <dd className={instance.restart_count ? 'text-warning tabular' : 'text-foreground tabular'}>
            {instance.restart_count ?? '—'}
          </dd>
        </div>
        <div className="flex min-w-0 flex-col gap-0.5 border-l px-4 py-3">
          <dt className="mono-label">Started</dt>
          <dd
            className="truncate text-foreground"
            title={instance.started_at ? dateTime(instance.started_at) : undefined}
          >
            {instance.started_at ? relativeTime(instance.started_at) : '—'}
          </dd>
        </div>
        <div className="flex min-w-0 flex-col gap-0.5 border-l px-4 py-3">
          <dt className="mono-label truncate">Host port</dt>
          <dd className="flex min-w-0 items-center gap-1 font-mono text-foreground tabular">
            {instance.host_port ?? '—'}
            {instance.host_port ? (
              <CopyButton
                value={`127.0.0.1:${instance.host_port}`}
                what={`host address of ${short}`}
                variant="ghost"
                className="size-5 [&_svg]:size-3"
              />
            ) : null}
          </dd>
        </div>
      </dl>
    </MetricCard>
  )
}

function Summary({ status }: { status: RuntimeStatus }) {
  const running = status.instances.filter((i) => i.state === 'running')
  const cpu = running.reduce((sum, i) => sum + (i.cpu_percent ?? 0), 0)
  const mem = running.reduce((sum, i) => sum + (i.memory_bytes ?? 0), 0)
  const items: [React.ReactNode, string][] = [
    [
      <>
        {running.length}
        <span className="text-foreground-lighter">/{status.desired_instances}</span>
      </>,
      'Running',
    ],
    [percent(cpu), 'CPU'],
    [bytes(mem), 'Memory'],
  ]
  return (
    <dl className="flex flex-wrap items-baseline gap-x-8 gap-y-1">
      {items.map(([value, label]) => (
        <div key={label} className="flex items-baseline gap-2 text-lg md:text-xl">
          <dt className="order-2 text-foreground-light">{label}</dt>
          <dd className="order-1 text-foreground tabular">{value}</dd>
        </div>
      ))}
    </dl>
  )
}

interface InstancesSectionProps {
  service: ServiceView
  status: RuntimeStatus | undefined
  updatedAt: number
  loading: boolean
  error: Error | null
  onRetry: () => void
  retrying: boolean
  onScale?: () => void
  refreshSeconds: number
}

/** Live instances: summary line + one metric card per container. */
export function InstancesSection({
  service,
  status,
  updatedAt,
  loading,
  error,
  onRetry,
  retrying,
  onScale,
  refreshSeconds,
}: InstancesSectionProps) {
  const history = useCpuHistory(status, updatedAt)
  const resume = useResumeService(service.name)

  const live = (
    <span className="inline-flex items-center gap-2 text-[12px] text-foreground-lighter">
      <StatusDot tone="success" pulse />
      Refreshing every {refreshSeconds}s
    </span>
  )

  let body: React.ReactNode
  if (service.type === 'cron_job') {
    body = (
      <EmptyState
        icon={<CalendarClock />}
        title="Cron jobs have no long-running instances"
        description={
          <>
            Each run starts a fresh container on schedule (<span className="font-mono">{service.schedule}</span>) and
            stops when done.
          </>
        }
        actions={
          <Button asChild size="tiny">
            <Link to={servicePath(service.name, '/jobs')}>View job runs</Link>
          </Button>
        }
      />
    )
  } else if (service.suspended) {
    body = (
      <EmptyState
        icon={<Pause />}
        title="Service is suspended"
        description="All instances are stopped. Resume the service to start them again."
        actions={
          <Button
            size="tiny"
            variant="primary"
            icon={<Play />}
            loading={resume.isPending}
            onClick={() =>
              resume.mutateAsync().then(
                () => toast.success(`${service.name} resumed`),
                (e: unknown) => toast.error('Could not resume', { description: errorMessage(e) }),
              )
            }
          >
            Resume service
          </Button>
        }
      />
    )
  } else if (error && !status) {
    body = <ErrorState error={error} title="Could not load instances" onRetry={onRetry} retrying={retrying} />
  } else if (loading && !status) {
    body = (
      <div
        className="grid gap-4 @min-[660px]:grid-cols-2 @min-[1020px]:grid-cols-3"
        aria-busy="true"
        aria-label="Loading instances"
      >
        {Array.from({ length: Math.min(service.instances, 3) }, (_, i) => (
          <div key={i} className="flex flex-col gap-4 rounded-lg border bg-surface-100 p-5">
            <Skeleton className="h-4 w-32" />
            <Skeleton className="h-7 w-24" />
            <Skeleton className="h-10 w-full" />
          </div>
        ))}
      </div>
    )
  } else if (status && status.instances.length === 0) {
    body = (
      <EmptyState
        icon={service.live_deploy_id ? <Boxes /> : <Rocket />}
        title={service.live_deploy_id ? 'No instances running' : 'Nothing deployed yet'}
        description={
          service.live_deploy_id
            ? 'The reconciler will start them again shortly. Check the logs of the latest deploy if this persists.'
            : service.state === 'deploying'
              ? 'The first deploy is in progress; instances appear here once it goes live.'
              : 'Instances appear here once a deploy goes live.'
        }
        actions={
          service.latest_deploy ? (
            <Button asChild size="tiny">
              <Link to={servicePath(service.name, `/deploys/${encodeURIComponent(service.latest_deploy.id)}`)}>
                View latest deploy
              </Link>
            </Button>
          ) : undefined
        }
      />
    )
  } else if (status) {
    body = (
      <div className="flex flex-col gap-4">
        <Summary status={status} />
        <div className="grid gap-4 @min-[660px]:grid-cols-2 @min-[1020px]:grid-cols-3">
          {status.instances.map((inst) => (
            <InstanceCard
              key={inst.container_id}
              instance={inst}
              samples={history[inst.container_id] ?? []}
              settingsPath={servicePath(service.name, '/settings')}
            />
          ))}
        </div>
      </div>
    )
  }

  const showLive = service.type !== 'cron_job' && !service.suspended && Boolean(status)

  return (
    <PageSection
      className="@container"
      title="Instances"
      description="Live CPU and memory of each container, against its limits."
      actions={
        <>
          {showLive && live}
          {onScale && service.type !== 'cron_job' && (
            <Button size="tiny" icon={<Boxes />} onClick={onScale} disabled={service.suspended}>
              Scale
            </Button>
          )}
        </>
      }
    >
      {body}
    </PageSection>
  )
}
