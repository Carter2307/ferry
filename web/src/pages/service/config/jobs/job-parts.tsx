import { CalendarClock, Hand } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { Badge } from '@/components/ui/badge'
import { ApiError, errorMessage } from '@/lib/api/client'
import { keys } from '@/lib/api/keys'
import { useCancelJob } from '@/lib/api/queries'
import type { JobRun, JobTrigger } from '@/lib/api/types'
import { shortId } from '@/lib/format'
import { cn } from '@/lib/utils'

/** `SCHEDULE` / `MANUAL` pill. */
export function JobTriggerBadge({ trigger }: { trigger: JobTrigger }) {
  return trigger === 'schedule' ? (
    <Badge variant="outline" className="gap-1">
      <CalendarClock aria-hidden="true" />
      Schedule
    </Badge>
  ) : (
    <Badge variant="outline" className="gap-1">
      <Hand aria-hidden="true" />
      Manual
    </Badge>
  )
}

/** The job's command, or "default command" when it runs the service's own command. */
export function JobCommand({ command, className }: { command: string | null; className?: string }) {
  return command ? (
    <code className={cn('font-mono text-[12.5px] text-foreground', className)} title={command}>
      {command}
    </code>
  ) : (
    <span className={cn('text-[13px] text-foreground-lighter italic', className)}>default command</span>
  )
}

/** Exit code: green 0, red otherwise, dash while unknown. */
export function ExitCode({ code }: { code: number | null }) {
  if (code === null) return <span className="text-foreground-lighter">—</span>
  return (
    <span className={cn('font-mono text-[12.5px] tabular-nums', code === 0 ? 'text-primary' : 'text-destructive')}>{code}</span>
  )
}

/**
 * Confirmation for `POST /api/v1/jobs/{id}/cancel`. The dialog closes as soon
 * as the user confirms: stopping the container can take ~10s, so progress
 * shows on the page (`useJobCanceling`) and in a toast. A 409 (the job
 * finished in the meantime) is reported as a notice, not an error.
 */
export function CancelJobDialog({
  job,
  open,
  onOpenChange,
}: {
  job: JobRun | null
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const cancel = useCancelJob()
  const qc = useQueryClient()
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      variant="danger"
      title={`Cancel job ${job ? shortId(job.id) : ''}?`}
      description={<p>The job’s container is stopped (SIGTERM, then SIGKILL after 10 seconds) and the run is marked canceled.</p>}
      confirmLabel="Cancel job"
      cancelLabel="Keep running"
      onConfirm={() => {
        if (!job) return
        const id = job.id
        const toastId = toast.loading(`Canceling job ${shortId(id)}…`)
        cancel.mutate(id, {
          onSuccess: () => {
            toast.success('Job canceled', { id: toastId })
          },
          onError: (e: unknown) => {
            if (e instanceof ApiError && e.isConflict) {
              toast.info('This job already finished', { id: toastId, description: e.message })
              void qc.invalidateQueries({ queryKey: keys.job(id) })
              void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === 'services' && q.queryKey[3] === 'jobs' })
              return
            }
            toast.error('Could not cancel the job', { id: toastId, description: errorMessage(e) })
          },
        })
      }}
    />
  )
}
