import * as React from 'react'
import { useNavigate } from 'react-router'
import { ArrowUpRight, Copy, EllipsisVertical, Pause, Play, Rocket, RotateCw, Trash2 } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { copyText } from '@/hooks/useCopy'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { ApiError, errorMessage } from '@/lib/api/client'
import {
  useDeleteService,
  useRestartService,
  useResumeService,
  useSuspendService,
  useTriggerDeploy,
} from '@/lib/api/queries'
import { sourceKind, type ServiceView } from '@/lib/api/types'
import { cn } from '@/lib/utils'
import { servicePath } from '@/pages/service/context'

type Dialog = 'suspend' | 'delete' | null

/** Stops clicks inside portaled dialogs from bubbling (through React) to a clickable row. */
function stop(e: React.SyntheticEvent) {
  e.stopPropagation()
}

/**
 * Kebab `⋮` menu of a service card / row: Open, Deploy, Restart,
 * Suspend | Resume, Copy URL, Delete (typed confirmation, force on 409).
 */
export function ServiceMenu({ service, className }: { service: ServiceView; className?: string }) {
  const { name } = service
  const navigate = useNavigate()
  const deploy = useTriggerDeploy(name)
  const restart = useRestartService(name)
  const suspend = useSuspendService(name)
  const resume = useResumeService(name)
  const remove = useDeleteService()
  const [dialog, setDialog] = React.useState<Dialog>(null)
  const triggerRef = React.useRef<HTMLButtonElement | null>(null)
  // A dialog opened from the menu takes focus itself: the menu must not
  // hand focus back to its trigger (which the dialog hides from AT).
  const openingDialog = React.useRef(false)
  const openDialog = (d: Exclude<Dialog, null>) => {
    openingDialog.current = true
    setDialog(d)
  }
  const onDialogOpenChange = (d: Exclude<Dialog, null>, open: boolean) => {
    setDialog(open ? d : null)
    // Back to the ⋮ button once the dialog is gone (it may have been deleted).
    if (!open) window.setTimeout(() => triggerRef.current?.focus(), 0)
  }
  const [conflict, setConflict] = React.useState(false)
  const [force, setForce] = React.useState(false)
  const forceId = React.useId()

  const kind = sourceKind(service)
  const suspended = service.suspended
  const canDeploy = !suspended && (kind !== 'upload' || service.latest_deploy !== null)
  const deployLabel =
    kind === 'git' ? 'Deploy latest commit' : kind === 'image' ? 'Pull & deploy image' : 'Redeploy last upload'

  const doDeploy = () =>
    deploy.mutate(
      {},
      {
        onSuccess: () => toast.success(`Deploy of ${name} queued`),
        onError: (e) => toast.error(`Could not deploy ${name}`, { description: errorMessage(e) }),
      },
    )
  const doRestart = () =>
    restart.mutate(undefined, {
      onSuccess: () => toast.success(`Restarting ${name}`),
      onError: (e) => toast.error(`Could not restart ${name}`, { description: errorMessage(e) }),
    })
  const doResume = () =>
    resume.mutate(undefined, {
      onSuccess: () => toast.success(`${name} resumed`),
      onError: (e) => toast.error(`Could not resume ${name}`, { description: errorMessage(e) }),
    })
  const doCopy = async (url: string) => {
    if (await copyText(url)) toast.success('URL copied', { description: url })
    else toast.error('Could not copy to the clipboard')
  }

  const openDelete = () => {
    setConflict(false)
    setForce(false)
    openDialog('delete')
  }

  const confirmDelete = async () => {
    try {
      await remove.mutateAsync({ ref: name, force })
      toast.success(`${name} deleted`)
    } catch (e) {
      if (e instanceof ApiError && e.isConflict) setConflict(true)
      throw e
    }
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            ref={triggerRef}
            variant="ghost"
            size="icon-tiny"
            icon={<EllipsisVertical />}
            aria-label={`Actions for ${name}`}
            className={cn('relative z-10 text-foreground-lighter', className)}
          />
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          className="w-56"
          onClick={stop}
          onCloseAutoFocus={(e) => {
            if (openingDialog.current) {
              e.preventDefault()
              openingDialog.current = false
            }
          }}
        >
          <DropdownMenuItem onSelect={() => void navigate(servicePath(name))}>
            <ArrowUpRight /> Open service
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={!canDeploy || deploy.isPending} onSelect={doDeploy}>
            <Rocket /> {deployLabel}
          </DropdownMenuItem>
          <DropdownMenuItem disabled={suspended || !service.live_deploy_id || restart.isPending} onSelect={doRestart}>
            <RotateCw /> Restart service
          </DropdownMenuItem>
          {suspended ? (
            <DropdownMenuItem disabled={resume.isPending} onSelect={doResume}>
              <Play /> Resume service
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem onSelect={() => openDialog('suspend')}>
              <Pause /> Suspend service…
            </DropdownMenuItem>
          )}
          {service.url && (
            <DropdownMenuItem onSelect={() => void doCopy(service.url ?? '')}>
              <Copy /> Copy URL
            </DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" onSelect={openDelete}>
            <Trash2 /> Delete service…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <span className="contents" onClick={stop} onKeyDown={stop}>
        <ConfirmDialog
          open={dialog === 'suspend'}
          onOpenChange={(o) => onDialogOpenChange('suspend', o)}
          variant="warning"
          title={`Suspend ${name}?`}
          description={
            <>
              <p>All instances are stopped and the proxy answers 503 until you resume the service.</p>
              {service.type === 'cron_job' && <p>Scheduled runs are skipped while it is suspended.</p>}
            </>
          }
          confirmLabel="Suspend service"
          onConfirm={() => suspend.mutateAsync().then(() => void toast.success(`${name} suspended`))}
        />
        <ConfirmDialog
          open={dialog === 'delete'}
          onOpenChange={(o) => onDialogOpenChange('delete', o)}
          title={`Delete ${name}?`}
          description={
            <>
              <p>
                This stops its containers and permanently deletes the service with its deploy history, environment
                variables and job runs. This cannot be undone.
              </p>
              {service.disk_mount_path && (
                <p>
                  The persistent disk mounted at <code className="font-mono text-foreground">{service.disk_mount_path}</code>{' '}
                  is deleted too.
                </p>
              )}
            </>
          }
          confirmText={name}
          confirmLabel={force ? 'Delete anyway' : 'Delete service'}
          onConfirm={confirmDelete}
        >
          {conflict && (
            <div className="flex items-start gap-2.5 rounded-md border border-warning-border bg-warning-soft p-3">
              <Checkbox id={forceId} checked={force} onCheckedChange={(c) => setForce(c === true)} className="mt-0.5" />
              <label htmlFor={forceId} className="flex flex-col gap-0.5 text-[13px]">
                <span className="font-medium text-foreground">Delete anyway</span>
                <span className="text-foreground-light">
                  Services that reference {name} will fail to deploy or restart until you remove the references.
                </span>
              </label>
            </div>
          )}
        </ConfirmDialog>
      </span>
    </>
  )
}
