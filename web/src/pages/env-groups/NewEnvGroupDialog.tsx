import * as React from 'react'
import { useNavigate } from 'react-router'
import { AlertTriangle } from 'lucide-react'
import { toast } from 'sonner'

import { KeyValueEditor } from '@/components/patterns/KeyValueEditor'
import { useKeyValueRows } from '@/components/patterns/kv-rows'
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
import { Input } from '@/components/ui/input'
import { errorMessage } from '@/lib/api/client'
import { useCreateEnvGroup, useEnvGroups } from '@/lib/api/queries'
import { plural } from '@/lib/format'

import { envGroupNameError } from './lib'

/** "New env group" modal: name + optional initial variables. */
export function NewEnvGroupDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate()
  const create = useCreateEnvGroup()
  const { data: groups } = useEnvGroups()
  const kv = useKeyValueRows([])
  const [name, setName] = React.useState('')
  const [touched, setTouched] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const nameId = React.useId()

  const taken = React.useMemo(() => new Set((groups ?? []).map((g) => g.name)), [groups])
  const trimmed = name.trim()
  const nameError = envGroupNameError(trimmed, taken)
  const shownNameError = touched || trimmed !== '' ? nameError : null

  const reset = () => {
    setName('')
    setTouched(false)
    setError(null)
    kv.reset([])
    create.reset()
  }

  const close = () => {
    onOpenChange(false)
    reset()
  }

  const submit = async () => {
    setTouched(true)
    if (nameError || !kv.valid || create.isPending) return
    setError(null)
    try {
      const group = await create.mutateAsync({ name: trimmed, vars: kv.vars })
      toast.success(`Env group ${group.name} created`, {
        description:
          group.vars.length > 0 ? plural(group.vars.length, 'variable') : 'Add variables, then link services.',
      })
      close()
      void navigate(`/env-groups/${encodeURIComponent(group.name)}`)
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (create.isPending) return
        if (o) onOpenChange(true)
        else close()
      }}
    >
      <DialogContent size="xl">
        <form
          noValidate
          className="flex min-h-0 flex-col"
          onSubmit={(e) => {
            e.preventDefault()
            void submit()
          }}
        >
          <DialogHeader>
            <DialogTitle>New env group</DialogTitle>
            <DialogDescription>
              Variables shared by every service linked to the group. A service’s own variables take precedence.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="gap-5">
            <div className="flex flex-col gap-1.5">
              <label htmlFor={nameId} className="text-[13px] font-medium text-foreground">
                Name
              </label>
              <Input
                id={nameId}
                mono
                autoFocus
                autoComplete="off"
                spellCheck={false}
                placeholder="shared-settings"
                maxLength={64}
                value={name}
                onChange={(e) => setName(e.target.value)}
                aria-invalid={shownNameError ? true : undefined}
                aria-describedby={`${nameId}-${shownNameError ? 'error' : 'hint'}`}
              />
              {shownNameError ? (
                <p id={`${nameId}-error`} role="alert" className="text-[12.5px] text-destructive">
                  {shownNameError}
                </p>
              ) : (
                <p id={`${nameId}-hint`} className="text-[12.5px] text-foreground-lighter">
                  Letters, digits, “-”, “_” and “.”.
                </p>
              )}
            </div>
            <div className="flex flex-col gap-2">
              <span className="text-[13px] font-medium text-foreground">
                Variables <span className="font-normal text-foreground-lighter">(optional)</span>
              </span>
              <KeyValueEditor rows={kv.rows} onChange={kv.setRows} errors={kv.errors} />
            </div>
            {error && (
              <div
                role="alert"
                className="flex items-start gap-2 rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] text-foreground"
              >
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
                <span className="break-words">{error}</span>
              </div>
            )}
          </DialogBody>
          <DialogFooter>
            <Button disabled={create.isPending} onClick={close}>
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              loading={create.isPending}
              disabled={(touched && Boolean(nameError)) || !kv.valid}
            >
              Create env group
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
