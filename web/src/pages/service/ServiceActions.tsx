import * as React from 'react'
import { ChevronDown, ExternalLink, Eraser, Pause, Play, Rocket, RotateCw } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { errorMessage } from '@/lib/api/client'
import { useRestartService, useResumeService, useSuspendService, useTriggerDeploy } from '@/lib/api/queries'
import { sourceKind, type ServiceView } from '@/lib/api/types'

/**
 * Primary actions of the service header: `Manual deploy | ⌄` split button
 * (clear cache & deploy, restart, suspend / resume) and "Open".
 */
export function ServiceActions({ service }: { service: ServiceView }) {
  const name = service.name
  const deploy = useTriggerDeploy(name)
  const restart = useRestartService(name)
  const suspend = useSuspendService(name)
  const resume = useResumeService(name)
  const [confirmSuspend, setConfirmSuspend] = React.useState(false)

  const kind = sourceKind(service)
  const suspended = service.suspended
  const busy = deploy.isPending || restart.isPending || suspend.isPending || resume.isPending

  const doDeploy = (clearCache: boolean) =>
    deploy.mutate(
      { clear_cache: clearCache },
      {
        onSuccess: () => toast.success(clearCache ? 'Deploy queued (build cache cleared)' : 'Deploy queued'),
        onError: (e) => toast.error('Could not start the deploy', { description: errorMessage(e) }),
      },
    )

  const doRestart = () =>
    restart.mutate(undefined, {
      onSuccess: () => toast.success(`Restarting ${name}`),
      onError: (e) => toast.error('Could not restart', { description: errorMessage(e) }),
    })

  const doResume = () =>
    resume.mutate(undefined, {
      onSuccess: () => toast.success(`${name} resumed`),
      onError: (e) => toast.error('Could not resume', { description: errorMessage(e) }),
    })

  return (
    <div className="flex flex-wrap items-center gap-2">
      {service.url && !suspended && (
        <Button asChild icon={<ExternalLink />}>
          <a href={service.url} target="_blank" rel="noreferrer">
            Open
            <span className="sr-only"> {service.url} (opens in a new tab)</span>
          </a>
        </Button>
      )}
      {suspended ? (
        <Button variant="primary" icon={<Play />} loading={resume.isPending} onClick={doResume}>
          Resume service
        </Button>
      ) : (
        <div className="flex items-center">
          <Button
            variant="primary"
            icon={<Rocket />}
            loading={deploy.isPending}
            disabled={busy}
            onClick={() => doDeploy(false)}
            className="rounded-r-none"
            title={kind === 'upload' ? `Redeploys the last upload. Push new code with: ferry up ${name}` : undefined}
          >
            Manual deploy
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="primary"
                size="icon"
                icon={<ChevronDown />}
                aria-label="More deploy actions"
                disabled={busy}
                className="-ml-px rounded-l-none border-l-primary-foreground/25"
              />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-60">
              <DropdownMenuLabel>Deploy</DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => doDeploy(false)}>
                <Rocket /> {kind === 'git' ? 'Deploy latest commit' : kind === 'image' ? 'Pull & deploy image' : 'Redeploy last upload'}
              </DropdownMenuItem>
              {kind !== 'image' && (
                <DropdownMenuItem onSelect={() => doDeploy(true)}>
                  <Eraser /> Clear build cache & deploy
                </DropdownMenuItem>
              )}
              <DropdownMenuSeparator />
              <DropdownMenuLabel>Runtime</DropdownMenuLabel>
              <DropdownMenuItem onSelect={doRestart} disabled={!service.live_deploy_id}>
                <RotateCw /> Restart service
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={() => setConfirmSuspend(true)}>
                <Pause /> Suspend service
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      )}
      <ConfirmDialog
        open={confirmSuspend}
        onOpenChange={setConfirmSuspend}
        variant="warning"
        title={`Suspend ${name}?`}
        description={
          <>
            <p>All instances are stopped and the proxy answers 503 until you resume the service.</p>
            {service.type === 'cron_job' && <p>Scheduled runs are skipped while suspended.</p>}
          </>
        }
        confirmLabel="Suspend service"
        onConfirm={() =>
          suspend.mutateAsync().then(() => {
            toast.success(`${name} suspended`)
          })
        }
      />
    </div>
  )
}
