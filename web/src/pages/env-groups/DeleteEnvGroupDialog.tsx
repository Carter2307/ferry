import * as React from 'react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { Callout } from '@/components/patterns/EmptyState'
import { Checkbox } from '@/components/ui/checkbox'
import { ApiError } from '@/lib/api/client'
import { useDeleteEnvGroup } from '@/lib/api/queries'
import type { EnvGroupView } from '@/lib/api/types'
import { plural } from '@/lib/format'

interface DeleteEnvGroupDialogProps {
  group: Pick<EnvGroupView, 'name' | 'services' | 'vars'>
  open: boolean
  onOpenChange: (open: boolean) => void
  onDeleted?: () => void
}

/**
 * Typed-name confirmation. While services are linked the server answers 409
 * unless `force`; `restart` then restarts the linked live services so they
 * drop the variables now instead of at their next restart.
 */
export function DeleteEnvGroupDialog({ group, open, onOpenChange, onDeleted }: DeleteEnvGroupDialogProps) {
  const del = useDeleteEnvGroup()
  const [conflict, setConflict] = React.useState<string | null>(null)
  const [force, setForce] = React.useState(false)
  const [restart, setRestart] = React.useState(false)
  const forceId = React.useId()
  const restartId = React.useId()
  const linked = group.services
  const needsForce = linked.length > 0 || conflict !== null

  const handleOpenChange = (next: boolean) => {
    if (next) {
      setConflict(null)
      setForce(false)
      setRestart(false)
    }
    onOpenChange(next)
  }

  const confirm = async () => {
    const forced = needsForce && force
    try {
      await del.mutateAsync({ ref: group.name, force: forced, restart: forced && restart })
    } catch (e) {
      if (e instanceof ApiError && e.isConflict) {
        setConflict(e.message)
        throw new Error('Services are still linked to this group. Tick “Delete anyway” to force it.')
      }
      throw e
    }
    toast.success(`Env group ${group.name} deleted`, {
      description: forced && restart && linked.length > 0 ? 'Linked live services are restarting.' : undefined,
    })
    onDeleted?.()
  }

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={handleOpenChange}
      title={`Delete env group ${group.name}`}
      description={
        <p>
          This permanently deletes the group and its {plural(group.vars.length, 'variable')}. Services that relied on
          them lose them at their next deploy or restart.
        </p>
      }
      confirmText={group.name}
      confirmLabel={needsForce && force ? 'Force delete' : 'Delete env group'}
      onConfirm={confirm}
    >
      {needsForce && (
        <>
          <Callout
            tone="warning"
            title={linked.length > 0 ? `Linked to ${plural(linked.length, 'service')}` : 'This group is still linked'}
          >
            {linked.length > 0 ? (
              <p>
                <span className="font-medium text-foreground">{linked.join(', ')}</span> will lose these variables.
                Unlink the group from them first, or delete it anyway.
              </p>
            ) : (
              <p className="break-words">{conflict}</p>
            )}
          </Callout>
          <div className="flex flex-col gap-3">
            <label htmlFor={forceId} className="flex cursor-pointer items-start gap-2.5 text-[13px] text-foreground">
              <Checkbox
                id={forceId}
                checked={force}
                onCheckedChange={(v) => {
                  setForce(v === true)
                  if (v !== true) setRestart(false)
                }}
                className="mt-0.5"
              />
              <span>
                Delete anyway <span className="text-foreground-light">(force)</span>
              </span>
            </label>
            <label
              htmlFor={restartId}
              className="flex cursor-pointer items-start gap-2.5 text-[13px] text-foreground has-disabled:cursor-not-allowed has-disabled:opacity-50"
            >
              <Checkbox
                id={restartId}
                checked={restart}
                disabled={!force}
                onCheckedChange={(v) => setRestart(v === true)}
                className="mt-0.5"
              />
              <span>
                Restart the linked services now{' '}
                <span className="text-foreground-light">(only live services whose variables change)</span>
              </span>
            </label>
          </div>
        </>
      )}
    </ConfirmDialog>
  )
}
