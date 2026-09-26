import * as React from 'react'
import { Link, useNavigate } from 'react-router'
import { CalendarClock, ChevronRight, ListChecks, Play, Square } from 'lucide-react'

import { CodeBlock } from '@/components/patterns/Copy'
import { TableErrorRow, TableSkeletonRows } from '@/components/patterns/DataTable'
import { EmptyState } from '@/components/patterns/EmptyState'
import { PageSection } from '@/components/patterns/Page'
import { JobStatusBadge } from '@/components/patterns/StatusBadge'
import { rowLinkProps } from '@/components/patterns/table-utils'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Hint } from '@/components/ui/tooltip'
import { useJobCanceling, useJobs } from '@/lib/api/queries'
import { isJobTerminal, type JobRun, type ServiceView } from '@/lib/api/types'
import { dateTime, relativeTime, shortId } from '@/lib/format'

import { useNow } from './config/hooks'
import { CancelJobDialog, ExitCode, JobCommand, JobTriggerBadge } from './config/jobs/job-parts'
import { jobDuration } from './config/jobs/job-utils'
import { RunJobDialog } from './config/jobs/RunJobDialog'
import { describeSchedule } from './config/settings/forms'
import { servicePath, useServiceOutlet } from './context'

const PAGE = 20
const MAX = 100

/** `/services/:name/jobs` — job runs (live), Run job, cancel. */
export function JobsPage() {
  const { service } = useServiceOutlet()
  return <JobsView key={service.id} service={service} />
}

function JobsView({ service }: { service: ServiceView }) {
  const name = service.name
  const navigate = useNavigate()
  const [limit, setLimit] = React.useState(PAGE)
  const jobs = useJobs(name, limit)
  const [runOpen, setRunOpen] = React.useState(false)
  const [toCancel, setToCancel] = React.useState<JobRun | null>(null)

  const list = jobs.data
  const running = Boolean(list?.some((j) => !isJobTerminal(j.status)))
  // tick every second while a job runs (durations), otherwise refresh relative times now and then
  const now = useNow(running ? 1000 : 30_000)
  const isCron = service.type === 'cron_job'
  const canRun = Boolean(service.live_deploy_id)

  const runButton = (
    <Hint label={canRun ? `Run a command on ${name}` : 'Deploy the service first: jobs use the live deploy’s image'}>
      <span className="inline-flex">
        <Button variant="primary" icon={<Play />} disabled={!canRun} onClick={() => setRunOpen(true)}>
          {isCron ? 'Run now' : 'Run job'}
        </Button>
      </span>
    </Hint>
  )

  return (
    <>
      <PageSection
        title="Jobs"
        description={
          isCron
            ? 'Scheduled and manual runs of this cron job. Each run starts a fresh container from the live deploy.'
            : 'One-off commands (migrations, scripts, backfills) in a fresh container with the service’s image and environment.'
        }
        // one CTA at a time: the empty state carries its own
        actions={list && list.length === 0 ? undefined : runButton}
      >
        {isCron && <ScheduleStrip service={service} />}

        {list && list.length === 0 ? (
          <EmptyState
            icon={<ListChecks />}
            title="No job runs yet"
            description={
              isCron
                ? 'Runs appear here when the schedule fires. You can also trigger one now.'
                : canRun
                  ? 'Run a command now, or start one from your terminal.'
                  : 'Deploy the service first: jobs run from the live deploy’s image.'
            }
            actions={canRun ? runButton : undefined}
          >
            <CodeBlock
              className="mt-2 max-w-md"
              prompt
              code={isCron ? `ferry run ${name}` : `ferry run ${name} -- <command>`}
            />
          </EmptyState>
        ) : (
          <Table aria-label={`Job runs of ${name}`}>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead className="w-[120px]">Status</TableHead>
                <TableHead>Job</TableHead>
                <TableHead className="hidden md:table-cell">Trigger</TableHead>
                <TableHead className="hidden sm:table-cell">Exit</TableHead>
                <TableHead className="hidden md:table-cell">Duration</TableHead>
                <TableHead className="hidden sm:table-cell">Started</TableHead>
                <TableHead className="w-[1%]">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {jobs.isPending && !jobs.error ? (
                <TableSkeletonRows columns={7} rows={4} />
              ) : jobs.error && !list ? (
                <TableErrorRow colSpan={7} error={jobs.error} />
              ) : (
                list?.map((job) => {
                  const to = servicePath(name, `/jobs/${job.id}`)
                  const active = !isJobTerminal(job.status)
                  return (
                    <TableRow key={job.id} {...rowLinkProps(() => void navigate(to))}>
                      <TableCell>
                        <JobStatusBadge status={job.status} />
                      </TableCell>
                      <TableCell className="w-full max-w-0">
                        <div className="flex min-w-0 flex-col gap-0.5">
                          <Link
                            to={to}
                            className="w-fit font-mono text-[12.5px] text-foreground underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                          >
                            {shortId(job.id)}
                          </Link>
                          <JobCommand command={job.command} className="block truncate text-foreground-light" />
                          <span className="text-[12px] text-foreground-lighter sm:hidden">
                            {relativeTime(job.started_at ?? job.created_at, now)} · {jobDuration(job, now)}
                          </span>
                        </div>
                      </TableCell>
                      <TableCell className="hidden md:table-cell">
                        <JobTriggerBadge trigger={job.trigger} />
                      </TableCell>
                      <TableCell className="hidden sm:table-cell">
                        <ExitCode code={job.exit_code} />
                      </TableCell>
                      <TableCell className="hidden font-mono text-[12.5px] text-foreground-light tabular-nums md:table-cell">
                        {jobDuration(job, now)}
                      </TableCell>
                      <TableCell className="hidden text-foreground-light sm:table-cell">
                        <time dateTime={job.started_at ?? job.created_at} title={dateTime(job.started_at ?? job.created_at)}>
                          {relativeTime(job.started_at ?? job.created_at, now)}
                        </time>
                      </TableCell>
                      <TableCell className="text-right">
                        {active ? (
                          <CancelJobButton job={job} onClick={() => setToCancel(job)} />
                        ) : (
                          <ChevronRight className="ml-auto size-4 text-foreground-muted" aria-hidden="true" />
                        )}
                      </TableCell>
                    </TableRow>
                  )
                })
              )}
            </TableBody>
          </Table>
        )}

        {list && list.length >= limit && limit < MAX && (
          <div className="flex justify-center">
            <Button onClick={() => setLimit((l) => Math.min(l + PAGE * 2, MAX))} loading={jobs.isFetching && list.length >= limit}>
              Show older runs
            </Button>
          </div>
        )}
      </PageSection>

      <RunJobDialog service={service} open={runOpen} onOpenChange={setRunOpen} />
      <CancelJobDialog job={toCancel} open={toCancel !== null} onOpenChange={(o) => !o && setToCancel(null)} />
    </>
  )
}

/** Row action: "Cancel", or a spinner while the cancel request is in flight. */
function CancelJobButton({ job, onClick }: { job: JobRun; onClick: () => void }) {
  const canceling = useJobCanceling(job.id)
  return (
    <Button
      size="tiny"
      variant="danger"
      icon={<Square />}
      loading={canceling}
      onClick={onClick}
      aria-label={canceling ? `Canceling job ${shortId(job.id)}` : `Cancel job ${shortId(job.id)}`}
    >
      {canceling ? 'Canceling…' : 'Cancel'}
    </Button>
  )
}

/** Cron jobs: the schedule in words, with a link to edit it. */
function ScheduleStrip({ service }: { service: ServiceView }) {
  const human = service.schedule ? describeSchedule(service.schedule) : null
  return (
    <div className="flex flex-col gap-2 rounded-lg border bg-surface-100 px-4 py-3 shadow-card sm:flex-row sm:items-center">
      <div className="flex min-w-0 flex-1 items-center gap-3">
        <span
          aria-hidden="true"
          className="flex size-8 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-lighter"
        >
          <CalendarClock className="size-4" />
        </span>
        <div className="flex min-w-0 flex-col">
          <span className="text-sm text-foreground">
            <code className="font-mono text-[13px]">{service.schedule ?? 'no schedule'}</code>
            {human && <span className="text-foreground-light"> · {human}</span>}
          </span>
          <span className="text-[12.5px] text-foreground-lighter">
            {service.suspended
              ? 'Suspended: scheduled runs are skipped until the service is resumed.'
              : !service.live_deploy_id
                ? 'Not deployed yet: the schedule starts after the first successful deploy.'
                : 'Evaluated in UTC. A run is skipped while the previous one is still running.'}
          </span>
        </div>
      </div>
      <Button asChild size="tiny" variant="ghost" className="self-start sm:self-auto">
        <Link to={servicePath(service.name, '/settings')}>Edit schedule</Link>
      </Button>
    </div>
  )
}
