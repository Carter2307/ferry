import * as React from 'react'
import { Link } from 'react-router'
import { CalendarClock, Loader2, Pause, Play, Radio, Rocket, ScrollText, XCircle } from 'lucide-react'
import { toast } from 'sonner'

import { EmptyState } from '@/components/patterns/EmptyState'
import { LogViewer } from '@/components/patterns/LogViewer'
import { PageSection } from '@/components/patterns/Page'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectLabel,
  SelectGroup,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Toggle } from '@/components/ui/toggle'
import { Hint } from '@/components/ui/tooltip'
import { errorMessage } from '@/lib/api/client'
import { streamPaths } from '@/lib/api/endpoints'
import { useResumeService, useServiceStatus } from '@/lib/api/queries'
import { isDeployActive, isDeployFailed, type ServiceView } from '@/lib/api/types'
import { useLogStream, type LogEntry } from '@/lib/api/useLogStream'
import { shortId } from '@/lib/format'

import { servicePath, useServiceOutlet } from './context'
import { instanceShortId } from './runtime/utils'

const TAIL_OPTIONS = ['100', '500', '1000', 'all'] as const
type TailOption = (typeof TAIL_OPTIONS)[number]
const ALL_INSTANCES = '__all__'

/** Lines sorted by timestamp (stable); returns the same array when already in order. */
function chronological(lines: LogEntry[]): LogEntry[] {
  const times = lines.map((l) => Date.parse(l.ts))
  let sorted = true
  for (let i = 1; i < times.length; i++) {
    if ((times[i] ?? 0) < (times[i - 1] ?? 0)) {
      sorted = false
      break
    }
  }
  if (sorted) return lines
  return lines
    .map((line, i) => ({ line, t: times[i] ?? 0 }))
    .sort((a, b) => a.t - b.t || a.line.seq - b.line.seq)
    .map((x) => x.line)
}

type NoLogsReason = 'cron' | 'suspended' | 'deploying' | 'failed' | 'not_deployed'

/** Why there are no runtime logs to stream, or null when there should be. */
function noLogsReason(service: ServiceView): NoLogsReason | null {
  if (service.type === 'cron_job') return 'cron'
  if (service.suspended) return 'suspended'
  if (service.live_deploy_id) return null
  const latest = service.latest_deploy
  if (latest && isDeployActive(latest.status)) return 'deploying'
  if (latest && isDeployFailed(latest.status)) return 'failed'
  return 'not_deployed'
}

function NoLogsState({ service, reason }: { service: ServiceView; reason: NoLogsReason }) {
  const resume = useResumeService(service.name)
  const latest = service.latest_deploy
  const deployLink = (label: string) =>
    latest ? (
      <Button asChild size="tiny" icon={<ScrollText />}>
        <Link to={servicePath(service.name, `/deploys/${encodeURIComponent(latest.id)}`)}>{label}</Link>
      </Button>
    ) : null

  if (reason === 'cron') {
    return (
      <EmptyState
        size="lg"
        icon={<CalendarClock />}
        title="Cron jobs have no runtime logs"
        description={
          <>
            <span className="font-mono">{service.name}</span> has no long-running instances: every scheduled run starts
            a container and keeps its own log.
          </>
        }
        actions={
          <Button asChild size="tiny" variant="primary">
            <Link to={servicePath(service.name, '/jobs')}>View job runs</Link>
          </Button>
        }
      />
    )
  }
  if (reason === 'suspended') {
    return (
      <EmptyState
        size="lg"
        icon={<Pause />}
        title="Service is suspended"
        description="Its instances are stopped, so there is nothing to stream. Resume it to start them again."
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
  }
  if (reason === 'deploying' && latest) {
    return (
      <EmptyState
        size="lg"
        icon={<Loader2 className="animate-spin" />}
        title="First deploy in progress"
        description={`Runtime logs appear once deploy ${shortId(latest.id)} goes live. Meanwhile, follow its build log.`}
        actions={deployLink('Follow the build log')}
      />
    )
  }
  if (reason === 'failed' && latest) {
    return (
      <EmptyState
        size="lg"
        icon={<XCircle />}
        title="Nothing is running: the latest deploy failed"
        description={latest.error ?? 'Check the build and deploy log to see what went wrong.'}
        actions={deployLink('View the deploy log')}
      />
    )
  }
  return (
    <EmptyState
      size="lg"
      icon={<Rocket />}
      title="Not deployed yet"
      description="Runtime logs stream here once a deploy goes live. Start one with Manual deploy."
      actions={
        <Button asChild size="tiny">
          <Link to={servicePath(service.name, '/deploys')}>View deploys</Link>
        </Button>
      }
    />
  )
}

function RuntimeLogs({ service }: { service: ServiceView }) {
  const [tail, setTail] = React.useState<TailOption>('500')
  const [live, setLive] = React.useState(true)
  const [instance, setInstance] = React.useState(ALL_INSTANCES)
  const status = useServiceStatus(service.name, { intervalMs: 10000 })

  const stream = useLogStream(streamPaths.serviceLogs(service.name), {
    follow: live,
    tail: tail === 'all' ? undefined : Number(tail),
    maxLines: 10000,
  })

  // instances from the runtime status plus any seen in the stream (e.g. replaced containers)
  const instances = React.useMemo(() => {
    const set = new Set<string>()
    for (const i of status.data?.instances ?? []) set.add(instanceShortId(i))
    for (const l of stream.lines) if (l.instance) set.add(l.instance)
    return [...set].sort()
  }, [status.data, stream.lines])

  // The history of each container arrives one after the other: merge it chronologically.
  const ordered = React.useMemo(() => chronological(stream.lines), [stream.lines])
  const lines = React.useMemo(
    () => (instance === ALL_INSTANCES ? ordered : ordered.filter((l) => l.instance === instance)),
    [ordered, instance],
  )

  const tailId = React.useId()
  const instanceId = React.useId()

  const toolbarExtra = (
    <div className="flex flex-wrap items-center gap-2">
      <label htmlFor={tailId} className="sr-only">
        Lines of history
      </label>
      <Select value={tail} onValueChange={(v) => setTail(v as TailOption)}>
        <SelectTrigger id={tailId} size="sm" className="min-w-[112px]">
          <SelectValue />
        </SelectTrigger>
        <SelectContent position="popper" align="start">
          <SelectGroup>
            <SelectLabel>History per instance</SelectLabel>
            {TAIL_OPTIONS.map((t) => (
              <SelectItem key={t} value={t}>
                {t === 'all' ? 'All lines' : `Last ${t}`}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
      {(instances.length > 1 || instance !== ALL_INSTANCES) && (
        <>
          <label htmlFor={instanceId} className="sr-only">
            Instance
          </label>
          <Select value={instance} onValueChange={setInstance}>
            <SelectTrigger id={instanceId} size="sm" className="min-w-[132px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper" align="start">
              <SelectGroup>
                <SelectLabel>Instance</SelectLabel>
                <SelectItem value={ALL_INSTANCES}>All instances</SelectItem>
                {instances.map((i) => (
                  <SelectItem key={i} value={i}>
                    <span className="font-mono">{i}</span>
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </>
      )}
      <Hint label={live ? 'Streaming new lines — click to pause' : 'Paused: showing a snapshot — click to stream'}>
        <Toggle
          size="default"
          pressed={live}
          onPressedChange={setLive}
          aria-label="Stream new lines"
          className="gap-1.5 px-2 text-[13px]"
        >
          <Radio />
          <span className="hidden sm:inline">{live ? 'Live' : 'Paused'}</span>
        </Toggle>
      </Hint>
    </div>
  )

  return (
    <LogViewer
      lines={lines}
      status={stream.status}
      error={stream.error}
      onClear={stream.clear}
      onRestart={stream.restart}
      showInstance="auto"
      downloadName={`${service.name}-logs`}
      height="max(360px, calc(100dvh - 380px))"
      toolbarExtra={toolbarExtra}
      label={`Runtime logs of ${service.name}`}
      emptyText={
        instance !== ALL_INSTANCES && stream.lines.length > 0
          ? `No lines from instance ${instance} yet.`
          : live
            ? 'Waiting for output… Your service has not logged anything recently.'
            : 'No log lines in this snapshot.'
      }
    />
  )
}

/** `/services/:name/logs` — runtime logs of every instance, merged. */
export function LogsPage() {
  const { service } = useServiceOutlet()
  const reason = noLogsReason(service)
  const empty = reason !== null
  return (
    <PageSection
      title="Logs"
      description={
        empty
          ? 'stdout and stderr of the running instances.'
          : 'stdout and stderr of the running instances, merged and streamed live.'
      }
    >
      {reason ? <NoLogsState service={service} reason={reason} /> : <RuntimeLogs key={service.id} service={service} />}
    </PageSection>
  )
}
