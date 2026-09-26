import * as React from 'react'
import { useNavigate } from 'react-router'
import { Play, TriangleAlert } from 'lucide-react'
import { toast } from 'sonner'

import { Callout } from '@/components/patterns/EmptyState'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import { errorMessage } from '@/lib/api/client'
import { useRunJob } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'
import { shortId } from '@/lib/format'

import { servicePath } from '../../context'

interface RunJobDialogProps {
  service: ServiceView
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Pre-filled command ("Run again"). */
  initialCommand?: string
}

/**
 * "Run job": a one-off command in a new container from the live deploy's
 * image. The command is required except for cron jobs (their own command).
 */
export function RunJobDialog({ service, open, onOpenChange, initialCommand = '' }: RunJobDialogProps) {
  const name = service.name
  const navigate = useNavigate()
  const run = useRunJob(name)
  const [command, setCommand] = React.useState(initialCommand)
  const [error, setError] = React.useState<string | null>(null)
  const [touched, setTouched] = React.useState(false)
  const inputId = React.useId()
  const hintId = `${inputId}-hint`
  const errorId = `${inputId}-error`

  const isCron = service.type === 'cron_job'
  const trimmed = command.trim()
  const missing = !isCron && trimmed === ''
  const shownError = error ?? (touched && missing ? 'Enter the command to run.' : null)

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    setTouched(true)
    setError(null)
    if (missing || run.isPending) return
    run.mutate(
      { command: trimmed === '' ? null : trimmed },
      {
        onSuccess: (job) => {
          onOpenChange(false)
          toast.success('Job started', { description: trimmed || 'Running the scheduled command' })
          void navigate(servicePath(name, `/jobs/${encodeURIComponent(job.id)}`), { state: { focus: 'heading' } })
        },
        onError: (err) => setError(errorMessage(err)),
      },
    )
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (run.isPending) return
        onOpenChange(o)
      }}
    >
      <DialogContent
        size="lg"
        onOpenAutoFocus={() => {
          setCommand(initialCommand)
          setError(null)
          setTouched(false)
        }}
      >
        <form onSubmit={submit} noValidate className="flex min-h-0 flex-col">
          <DialogHeader>
            <DialogTitle>Run a job on {name}</DialogTitle>
            <DialogDescription>
              Runs in a new container from the live deploy
              {service.live_deploy_id ? ` (${shortId(service.live_deploy_id)})` : ''}, with the service’s environment and
              private network{service.disk_mount_path ? ' and its disk' : ''}. No port is exposed.
            </DialogDescription>
          </DialogHeader>
          <DialogBody>
            {!service.live_deploy_id && (
              <Callout tone="warning" icon={<TriangleAlert />}>
                {name} has no live deploy yet. Deploy it first: jobs use the live deploy’s image.
              </Callout>
            )}
            <div className="flex flex-col gap-2">
              <label htmlFor={inputId} className="text-sm font-medium text-foreground">
                Command{isCron && <span className="font-normal text-foreground-lighter"> (optional)</span>}
              </label>
              <Textarea
                id={inputId}
                mono
                rows={3}
                autoFocus
                spellCheck={false}
                autoComplete="off"
                placeholder={isCron ? (service.start_command ?? 'the scheduled command') : 'npm run migrate'}
                value={command}
                aria-invalid={shownError ? true : undefined}
                aria-describedby={shownError ? `${hintId} ${errorId}` : hintId}
                onChange={(e) => {
                  setCommand(e.target.value)
                  setError(null)
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault()
                    e.currentTarget.form?.requestSubmit()
                  }
                }}
              />
              <p id={hintId} className="text-[13px] text-foreground-light">
                {isCron
                  ? 'Leave empty to run the job’s own command now, outside the schedule.'
                  : 'Runs with /bin/sh -c, so pipes and && work.'}{' '}
                <span className="hidden sm:inline">⌘/Ctrl + Enter to run.</span>
              </p>
              {shownError && (
                <p id={errorId} role="alert" className="text-[13px] text-destructive">
                  {shownError}
                </p>
              )}
            </div>
          </DialogBody>
          <DialogFooter>
            <Button type="button" onClick={() => onOpenChange(false)} disabled={run.isPending}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" icon={<Play />} loading={run.isPending}>
              Run job
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
