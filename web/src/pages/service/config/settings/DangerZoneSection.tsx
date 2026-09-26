import * as React from 'react'
import { Pause, Play, Trash2 } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { PageSection } from '@/components/patterns/Page'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { ApiError, errorMessage } from '@/lib/api/client'
import { useDeleteService, useResumeService, useSuspendService } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'
import { cn } from '@/lib/utils'

/** One Studio "availability" row: title + text on the left, action on the right. */
function ActionRow({
  title,
  description,
  action,
  className,
}: {
  title: React.ReactNode
  description: React.ReactNode
  action: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center sm:justify-between md:px-6', className)}>
      <div className="flex min-w-0 flex-col gap-0.5">
        <p className="text-sm font-medium text-foreground">{title}</p>
        <div className="text-[13px] text-foreground-light">{description}</div>
      </div>
      <div className="shrink-0">{action}</div>
    </div>
  )
}

/** Suspend / resume, and delete with typed confirmation (+ force when referenced). */
export function DangerZoneSection({ service, onDeleted }: { service: ServiceView; onDeleted: () => void }) {
  const name = service.name
  const suspend = useSuspendService(name)
  const resume = useResumeService(name)
  const del = useDeleteService()
  const [confirmSuspend, setConfirmSuspend] = React.useState(false)
  const [confirmDelete, setConfirmDelete] = React.useState(false)
  const [conflict, setConflict] = React.useState<string | null>(null)
  const [force, setForce] = React.useState(false)
  const forceId = React.useId()
  const isCron = service.type === 'cron_job'

  const doResume = () =>
    resume.mutate(undefined, {
      onSuccess: () => toast.success(`${name} resumed`, { description: 'Instances are starting from the live deploy.' }),
      onError: (e) => toast.error(`Could not resume ${name}`, { description: errorMessage(e) }),
    })

  return (
    <PageSection id="danger" title="Danger zone" description="Actions that stop the service or remove it for good.">
      <Card className="gap-0 divide-y">
        {service.suspended ? (
          <ActionRow
            title="Resume service"
            description={
              isCron
                ? 'Scheduled runs start again at the next matching time.'
                : 'Starts the instances of the live deploy again and routes traffic back to them.'
            }
            action={
              <Button icon={<Play />} loading={resume.isPending} onClick={doResume}>
                Resume service
              </Button>
            }
          />
        ) : (
          <ActionRow
            title="Suspend service"
            description={
              isCron
                ? 'Scheduled runs are skipped until you resume it. Settings, variables and deploys are kept.'
                : 'Stops every instance; the proxy answers 503 until you resume. Settings, variables and deploys are kept.'
            }
            action={
              <Button icon={<Pause />} loading={suspend.isPending} onClick={() => setConfirmSuspend(true)}>
                Suspend service
              </Button>
            }
          />
        )}
      </Card>

      <Card className="gap-0 border-destructive-border">
        <ActionRow
          className="bg-destructive-soft/60"
          title="Delete service"
          description={
            <>
              Removes {name}, its containers, images, deploy history{service.disk_mount_path ? ', its disk' : ''} and variables.
              This cannot be undone.
            </>
          }
          action={
            <Button
              variant="danger"
              icon={<Trash2 />}
              onClick={() => {
                setConflict(null)
                setForce(false)
                setConfirmDelete(true)
              }}
            >
              Delete service
            </Button>
          }
        />
      </Card>

      <ConfirmDialog
        open={confirmSuspend}
        onOpenChange={setConfirmSuspend}
        variant="warning"
        title={`Suspend ${name}?`}
        description={
          <p>
            {isCron
              ? 'Scheduled runs are skipped while it is suspended.'
              : 'All instances are stopped and the proxy answers 503 until you resume the service.'}
          </p>
        }
        confirmLabel="Suspend service"
        onConfirm={() =>
          suspend.mutateAsync().then(() => {
            toast.success(`${name} suspended`)
          })
        }
      />

      <ConfirmDialog
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title={`Delete ${name}?`}
        description={
          <p>
            This permanently removes the service, its deploys, logs and variables
            {service.custom_domains.length > 0 && `, and frees ${service.custom_domains.join(', ')}`}.
          </p>
        }
        confirmText={name}
        confirmLabel={force ? 'Delete anyway' : 'Delete service'}
        onConfirm={() =>
          del
            .mutateAsync({ ref: name, force })
            .then(() => {
              toast.success(`Deleted ${name}`)
              onDeleted()
            })
            .catch((e: unknown) => {
              if (e instanceof ApiError && e.isConflict) setConflict(e.message)
              throw e
            })
        }
      >
        {conflict && (
          <label
            htmlFor={forceId}
            className="flex cursor-pointer items-start gap-2.5 rounded-md border border-warning-border bg-warning-soft p-3 text-[13px]"
          >
            <Checkbox id={forceId} checked={force} onCheckedChange={(c) => setForce(c === true)} className="mt-0.5" />
            <span className="flex flex-col gap-0.5">
              <span className="font-medium text-foreground">Delete even if referenced</span>
              <span className="text-foreground-light">
                Services that reference {name} with <code className="font-mono">{'${{service.…}}'}</code> will fail their
                next deploy or restart until you remove the reference.
              </span>
            </span>
          </label>
        )}
      </ConfirmDialog>
    </PageSection>
  )
}
