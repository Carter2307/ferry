import * as React from 'react'
import { Boxes, ChevronDown, Eraser, Pause, Play, Rocket, RotateCw, SquareTerminal } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { errorMessage } from '@/lib/api/client'
import { useRestartService, useResumeService, useSuspendService, useTriggerDeploy } from '@/lib/api/queries'
import { isPublicHttp, sourceKind, type ServiceView } from '@/lib/api/types'
import { plural } from '@/lib/format'

import { deployActionLabel } from './utils'

/** Wider description column: the controls on the right are compact. */
const ROW = 'md:grid-cols-[minmax(0,1fr)_auto] md:gap-10'

interface AvailabilityCardProps {
  service: ServiceView
  onScale: () => void
  onRunJob: () => void
}

/**
 * Studio "Project availability" card: one row per runtime action — manual
 * deploy (optionally clearing the build cache), restart split button, scale,
 * one-off job and suspend / resume.
 */
export function AvailabilityCard({ service, onScale, onRunJob }: AvailabilityCardProps) {
  const name = service.name
  const kind = sourceKind(service)
  const deploy = useTriggerDeploy(name)
  const restart = useRestartService(name)
  const suspend = useSuspendService(name)
  const resume = useResumeService(name)
  const [clearCache, setClearCache] = React.useState(false)
  const [confirmSuspend, setConfirmSuspend] = React.useState(false)
  const clearCacheId = React.useId()

  const suspended = service.suspended
  const isCron = service.type === 'cron_job'
  const canClearCache = kind !== 'image'

  const doDeploy = (clear: boolean) =>
    deploy.mutate(
      { clear_cache: clear },
      {
        onSuccess: () => {
          toast.success(clear ? 'Deploy queued (build cache cleared)' : 'Deploy queued')
          setClearCache(false)
        },
        onError: (e) => toast.error('Could not start the deploy', { description: errorMessage(e) }),
      },
    )

  const doRestart = () =>
    restart.mutate(undefined, {
      onSuccess: () => toast.success(`Restarting ${name}`, { description: 'Fresh containers from the live image.' }),
      onError: (e) => toast.error('Could not restart', { description: errorMessage(e) }),
    })

  const doResume = () =>
    resume.mutate(undefined, {
      onSuccess: () => toast.success(`${name} resumed`),
      onError: (e) => toast.error('Could not resume', { description: errorMessage(e) }),
    })

  const suspendedNote = suspended ? 'Resume the service first.' : undefined

  return (
    <>
      <FormCard asDiv>
        <FormRow
          className={ROW}
          label="Manual deploy"
          description={
            kind === 'git' ? (
              <>
                Build and deploy the latest commit of{' '}
                <span className="font-mono text-foreground">{service.branch}</span>.
              </>
            ) : kind === 'image' ? (
              <>
                Pull <span className="font-mono break-all text-foreground">{service.image}</span> again and deploy it.
              </>
            ) : (
              <>
                Rebuild the last uploaded archive. Push new code with{' '}
                <span className="font-mono text-foreground">ferry up {name}</span>.
              </>
            )
          }
        >
          <div className="flex flex-wrap items-center gap-x-4 gap-y-3 md:justify-end">
            {canClearCache && (
              <label
                htmlFor={clearCacheId}
                className="flex cursor-pointer items-center gap-2 text-[13px] text-foreground-light"
              >
                <Checkbox
                  id={clearCacheId}
                  checked={clearCache}
                  onCheckedChange={(c) => setClearCache(c === true)}
                  disabled={suspended}
                />
                Clear build cache
              </label>
            )}
            <Button
              variant="primary"
              icon={clearCache ? <Eraser /> : <Rocket />}
              loading={deploy.isPending}
              disabled={suspended}
              title={suspendedNote}
              onClick={() => doDeploy(clearCache)}
            >
              {kind === 'git' ? 'Deploy latest commit' : kind === 'image' ? 'Pull & deploy' : 'Redeploy upload'}
            </Button>
          </div>
        </FormRow>

        <FormRow
          className={ROW}
          label="Restart service"
          description="Start fresh containers from the live image with the current environment and settings. Nothing is rebuilt."
        >
          <div className="flex md:justify-end">
            <div className="flex items-center">
              <Button
                icon={<RotateCw />}
                loading={restart.isPending}
                disabled={suspended || !service.live_deploy_id}
                title={suspendedNote ?? (!service.live_deploy_id ? 'Nothing is live yet' : undefined)}
                onClick={doRestart}
                className="rounded-r-none"
              >
                Restart service
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    size="icon"
                    icon={<ChevronDown />}
                    aria-label="More restart options"
                    disabled={suspended || restart.isPending || deploy.isPending}
                    className="-ml-px rounded-l-none"
                  />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-64">
                  <DropdownMenuItem onSelect={doRestart} disabled={!service.live_deploy_id}>
                    <RotateCw /> Restart with the live image
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => doDeploy(false)}>
                    <Rocket /> {kind === 'git' ? 'Rebuild from source' : deployActionLabel(service)}
                  </DropdownMenuItem>
                  {canClearCache && (
                    <DropdownMenuItem onSelect={() => doDeploy(true)}>
                      <Eraser /> Clear build cache & rebuild
                    </DropdownMenuItem>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
        </FormRow>

        <FormRow
          className={ROW}
          label="Scale"
          description={
            isCron
              ? 'Cron jobs always run as a single container per run.'
              : service.disk_mount_path
                ? 'Services with a persistent disk run exactly 1 instance.'
                : `Currently ${plural(service.instances, 'instance')}. Each instance is its own container${isPublicHttp(service.type) ? ' behind the proxy' : ''}.`
          }
        >
          <div className="flex md:justify-end">
            <Button
              icon={<Boxes />}
              onClick={onScale}
              disabled={isCron || suspended}
              title={isCron ? 'Not available for cron jobs' : suspendedNote}
            >
              Scale instances
            </Button>
          </div>
        </FormRow>

        <FormRow
          className={ROW}
          label="Run a one-off job"
          description={
            isCron
              ? 'Trigger a run now, outside the schedule, or run another command.'
              : 'Run a command (migrations, scripts…) in a new container from the live image.'
          }
        >
          <div className="flex md:justify-end">
            <Button
              icon={<SquareTerminal />}
              onClick={onRunJob}
              disabled={suspended || !service.live_deploy_id}
              title={suspendedNote ?? (!service.live_deploy_id ? 'Deploy the service first' : undefined)}
            >
              Run job
            </Button>
          </div>
        </FormRow>

        {suspended ? (
          <FormRow
            className={ROW}
            label="Resume service"
            description={`Start the instances again from the live deploy.${isPublicHttp(service.type) ? ' The proxy routes traffic to them as soon as they are healthy.' : ''}`}
          >
            <div className="flex md:justify-end">
              <Button variant="primary" icon={<Play />} loading={resume.isPending} onClick={doResume}>
                Resume service
              </Button>
            </div>
          </FormRow>
        ) : (
          <FormRow
            className={ROW}
            label="Suspend service"
            description={
              isCron
                ? 'Skip scheduled runs until you resume it. Settings, environment and deploys are kept.'
                : `Stop every instance${isPublicHttp(service.type) ? '; the proxy answers 503' : ''} until you resume it. Settings, environment and deploys are kept.`
            }
          >
            <div className="flex md:justify-end">
              <Button icon={<Pause />} loading={suspend.isPending} onClick={() => setConfirmSuspend(true)}>
                Suspend service
              </Button>
            </div>
          </FormRow>
        )}
      </FormCard>

      <ConfirmDialog
        open={confirmSuspend}
        onOpenChange={setConfirmSuspend}
        variant="warning"
        title={`Suspend ${name}?`}
        description={
          <>
            <p>
              All instances are stopped{isPublicHttp(service.type) ? ' and the proxy answers 503' : ''} until you resume
              the service.
            </p>
            {isCron && <p>Scheduled runs are skipped while suspended.</p>}
          </>
        }
        confirmLabel="Suspend service"
        onConfirm={() => suspend.mutateAsync().then(() => void toast.success(`${name} suspended`))}
      />
    </>
  )
}
