import * as React from 'react'
import { toast } from 'sonner'

import { Callout } from '@/components/patterns/EmptyState'
import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { Checkbox } from '@/components/ui/checkbox'
import { ApiError } from '@/lib/api/client'
import { useDeleteDatastore } from '@/lib/api/queries'
import type { DatastoreView } from '@/lib/api/types'

import type { DatastoreReference } from './useDatastoreReferences'

interface DeleteDatastoreDialogProps {
  datastore: Pick<DatastoreView, 'name' | 'kind'>
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Known references (detail page); the list page relies on the server's 409. */
  references?: DatastoreReference[]
  onDeleted?: () => void
}

/**
 * Typed-name confirmation. While services reference the datastore the server
 * answers 409; the dialog then shows why and offers a forced delete.
 */
export function DeleteDatastoreDialog({
  datastore,
  open,
  onOpenChange,
  references,
  onDeleted,
}: DeleteDatastoreDialogProps) {
  const del = useDeleteDatastore()
  const [conflict, setConflict] = React.useState<string | null>(null)
  const [force, setForce] = React.useState(false)
  const forceId = React.useId()
  const referenced = (references?.length ?? 0) > 0
  const needsForce = referenced || conflict !== null

  const handleOpenChange = (next: boolean) => {
    if (next) {
      setConflict(null)
      setForce(false)
    }
    onOpenChange(next)
  }

  const confirm = async () => {
    try {
      await del.mutateAsync({ ref: datastore.name, force: needsForce && force })
    } catch (e) {
      if (e instanceof ApiError && e.isConflict) {
        setConflict(e.message)
        throw new Error('Services still reference this datastore. Tick “Delete anyway” to force it.')
      }
      throw e
    }
    toast.success(`Datastore ${datastore.name} deleted`)
    onDeleted?.()
  }

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={handleOpenChange}
      title={`Delete datastore ${datastore.name}`}
      description={
        <p>
          This stops the container and permanently deletes its volume.{' '}
          <strong className="font-medium text-foreground">All data in {datastore.name} is lost</strong> and can’t be
          recovered.
        </p>
      }
      confirmText={datastore.name}
      confirmLabel={needsForce && force ? 'Force delete' : 'Delete datastore'}
      onConfirm={confirm}
    >
      {needsForce && (
        <Callout tone="warning" title="This datastore is still referenced">
          {referenced ? (
            <ul className="mt-1 flex flex-col gap-0.5">
              {references?.map((r) => (
                <li key={`${r.service.name}-${r.group ?? ''}`}>
                  <span className="font-medium text-foreground">{r.service.name}</span>{' '}
                  <span className="font-mono text-[12px]">({r.keys.join(', ')})</span>
                  {r.group && <span> via env group {r.group}</span>}
                </li>
              ))}
            </ul>
          ) : (
            <p className="break-words">{conflict}</p>
          )}
          <p className="mt-1.5">
            These services will fail their next deploy or restart until you remove the references.
          </p>
        </Callout>
      )}
      {needsForce && (
        <label htmlFor={forceId} className="flex cursor-pointer items-start gap-2.5 text-[13px] text-foreground">
          <Checkbox id={forceId} checked={force} onCheckedChange={(v) => setForce(v === true)} className="mt-0.5" />
          <span>
            Delete anyway{' '}
            <span className="text-foreground-light">(force — the referencing services keep broken references)</span>
          </span>
        </label>
      )}
    </ConfirmDialog>
  )
}
