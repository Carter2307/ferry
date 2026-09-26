import * as React from 'react'
import { Link, useLocation, useParams } from 'react-router'
import { ArrowLeft, ListChecks, RotateCcw, Square, TriangleAlert } from 'lucide-react'

import { CopyButton } from '@/components/patterns/Copy'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { LogViewer } from '@/components/patterns/LogViewer'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { PageSection } from '@/components/patterns/Page'
import { JobStatusBadge } from '@/components/patterns/StatusBadge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { ApiError } from '@/lib/api/client'
import { streamPaths } from '@/lib/api/endpoints'
import { useJob, useJobCanceling } from '@/lib/api/queries'
import { isJobTerminal, type JobRun, type ServiceView } from '@/lib/api/types'
import { useLogStream } from '@/lib/api/useLogStream'
import { dateTime, relativeTime, shortId } from '@/lib/format'
import { cn } from '@/lib/utils'

import { useNow } from './config/hooks'
import { CancelJobDialog, ExitCode, JobCommand, JobTriggerBadge } from './config/jobs/job-parts'
import { jobDuration } from './config/jobs/job-utils'
import { RunJobDialog } from './config/jobs/RunJobDialog'
import { servicePath, useServiceOutlet } from './context'

/** `/services/:name/jobs/:jobId` — job details + its log (followed while it runs). */
export function JobDetailPage() {
  const { service } = useServiceOutlet()
  const { jobId = '' } = useParams()
  return <JobDetailView key={jobId} service={service} jobId={jobId} />
}

/** Navigation state set by RunJobDialog: `{ focus: 'heading' }`. */
function wantsHeadingFocus(state: unknown): boolean {
  return typeof state === 'object' && state !== null && 'focus' in state && state.focus === 'heading'
}

function BackLink({ name }: { name: string }) {
  return (
    <Link
      to={servicePath(name, '/jobs')}
      className="inline-flex w-fit items-center gap-1.5 rounded-sm text-[13px] text-foreground-lighter outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
    >
      <ArrowLeft className="size-3.5" aria-hidden="true" />
      All jobs
    </Link>
  )
}

function JobDetailView({ service, jobId }: { service: ServiceView; jobId: string }) {
  const name = service.name
  const job = useJob(jobId)
  const [cancelOpen, setCancelOpen] = React.useState(false)
  const [rerunOpen, setRerunOpen] = React.useState(false)

  const data = job.data
  const active = data ? !isJobTerminal(data.status) : false
  const canceling = useJobCanceling(jobId) && active
  const headingRef = React.useRef<HTMLHeadingElement>(null)

  const location = useLocation()
  const focusHeading = wantsHeadingFocus(location.state)

  // Arriving from "Run job" (dialog closed, route changed): put focus on the page heading
  // instead of leaving it on <body>. Deferred so it runs after the dialog's own focus restore.
  React.useEffect(() => {
    if (!focusHeading) return
    const t = window.setTimeout(() => {
      const active = document.activeElement
      if (!active || active === document.body) headingRef.current?.focus()
    }, 0)
    return () => window.clearTimeout(t)
  }, [focusHeading])
  const now = useNow(active ? 1000 : 30_000)
  const logs = useLogStream(data ? streamPaths.jobLogs(data.id) : null, { follow: true })

  if (job.error && !data) {
    const notFound = job.error instanceof ApiError && job.error.isNotFound
    return (
      <div className="flex flex-col gap-4">
        <BackLink name={name} />
        {notFound ? (
          <EmptyState
            size="lg"
            icon={<ListChecks />}
            title="Job not found"
            description={`No job run ${jobId} exists. Old runs are pruned automatically.`}
            actions={
              <Button asChild variant="primary">
                <Link to={servicePath(name, '/jobs')}>All jobs</Link>
              </Button>
            }
          />
        ) : (
          <ErrorState error={job.error} title="Could not load the job" onRetry={() => void job.refetch()} retrying={job.isRefetching} />
        )}
      </div>
    )
  }

  return (
    <div className="flex flex-col">
      <header className="mb-6 flex flex-col gap-3">
        <BackLink name={name} />
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex min-w-0 flex-wrap items-center gap-2.5">
            <h2
              ref={headingRef}
              tabIndex={-1}
              className="flex items-baseline gap-2 rounded-sm text-lg font-medium text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring md:text-xl"
            >
              Job run
              <span className="font-mono text-[14px] font-normal text-foreground-light">{shortId(jobId)}</span>
            </h2>
            {data ? <JobStatusBadge status={data.status} /> : <Skeleton className="h-5 w-20 rounded-full" />}
            <CopyButton value={jobId} what="job ID" variant="ghost" />
          </div>
          {data && (
            <div className="flex flex-wrap items-center gap-2">
              {active && (
                <Button variant="danger" icon={<Square />} loading={canceling} onClick={() => setCancelOpen(true)}>
                  {canceling ? 'Canceling…' : 'Cancel job'}
                </Button>
              )}
              {!active && service.live_deploy_id && (
                <Button icon={<RotateCcw />} onClick={() => setRerunOpen(true)}>
                  Run again
                </Button>
              )}
            </div>
          )}
        </div>
      </header>

      {data && data.service_id !== service.id && (
        <Callout tone="warning" icon={<TriangleAlert />} className="mb-6">
          This job belongs to another service.
        </Callout>
      )}

      <JobFacts job={data} now={now} />

      {data?.error && (
        <Callout
          tone={data.status === 'canceled' ? 'warning' : 'destructive'}
          icon={<TriangleAlert />}
          title={data.status === 'canceled' ? 'The job was canceled' : 'The job failed'}
          className="mt-4"
        >
          <span className="font-mono text-[12.5px] break-words">{data.error}</span>
        </Callout>
      )}

      <PageSection title="Logs" className="mt-10">
        <LogViewer
          lines={logs.lines}
          status={logs.status}
          error={logs.error}
          onClear={logs.clear}
          onRestart={logs.restart}
          showInstance={false}
          downloadName={`${name}-${jobId}`}
          height="min(60dvh, 560px)"
          label={`Log of job ${jobId}`}
          emptyText={
            !data
              ? 'Loading…'
              : data.status === 'pending'
                ? 'Waiting for the container to start…'
                : active
                  ? 'No output yet.'
                  : 'This job printed nothing.'
          }
        />
      </PageSection>

      {data && (
        <>
          <CancelJobDialog job={data} open={cancelOpen} onOpenChange={setCancelOpen} />
          <RunJobDialog
            service={service}
            open={rerunOpen}
            onOpenChange={setRerunOpen}
            initialCommand={data.command ?? ''}
          />
        </>
      )}
    </div>
  )
}

/** Supabase "service health" style: one bordered container split into labelled cells. */
function JobFacts({ job, now }: { job: JobRun | undefined; now: number }) {
  const cells: { label: string; value: React.ReactNode; wide?: boolean }[] = job
    ? [
        { label: 'Trigger', value: <JobTriggerBadge trigger={job.trigger} /> },
        { label: 'Exit code', value: <ExitCode code={job.exit_code} /> },
        {
          label: 'Duration',
          value: <span className="font-mono text-[13px] tabular-nums">{jobDuration(job, now)}</span>,
        },
        {
          label: 'Started',
          value: job.started_at ? (
            <time dateTime={job.started_at} title={dateTime(job.started_at)}>
              {relativeTime(job.started_at, now)}
            </time>
          ) : (
            <span className="text-foreground-lighter">not yet</span>
          ),
        },
        { label: 'Command', value: <JobCommand command={job.command} className="break-all" />, wide: true },
        {
          label: 'Image',
          value: job.image ? (
            <code className="font-mono text-[12.5px] break-all text-foreground">{job.image}</code>
          ) : (
            <span className="text-foreground-lighter">—</span>
          ),
          wide: true,
        },
      ]
    : []

  return (
    // gap-px over a border-coloured backdrop draws the 1px separators between cells
    <div
      className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border shadow-card md:grid-cols-4"
      aria-busy={!job || undefined}
    >
      {job
        ? cells.map((c) => (
            <div key={c.label} className={cn('flex min-w-0 flex-col gap-1.5 bg-surface-100 px-4 py-3.5', c.wide && 'col-span-2')}>
              <MonoLabel>{c.label}</MonoLabel>
              <div className="min-w-0 text-sm text-foreground">{c.value}</div>
            </div>
          ))
        : Array.from({ length: 4 }, (_, i) => (
            <div key={i} className="flex flex-col gap-2 bg-surface-100 px-4 py-3.5">
              <Skeleton className="h-3 w-16" />
              <Skeleton className="h-4 w-24" />
            </div>
          ))}
    </div>
  )
}
