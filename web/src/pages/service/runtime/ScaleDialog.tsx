import * as React from 'react'
import { Minus, Plus } from 'lucide-react'
import { toast } from 'sonner'

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
import { useScaleService } from '@/lib/api/queries'
import { isPublicHttp, type ServiceView } from '@/lib/api/types'
import { plural } from '@/lib/format'

/** Server limit (`validate::MAX_INSTANCES`). */
const MAX_INSTANCES = 50

interface ScaleDialogProps {
  service: ServiceView
  open: boolean
  onOpenChange: (open: boolean) => void
}

function ScaleForm({ service, onDone }: { service: ServiceView; onDone: () => void }) {
  const scale = useScaleService(service.name)
  const [value, setValue] = React.useState(String(service.instances))
  const [error, setError] = React.useState<string | null>(null)
  const inputId = React.useId()
  const hintId = React.useId()

  const max = service.disk_mount_path ? 1 : MAX_INSTANCES
  const n = Number(value)
  const valid = Number.isInteger(n) && n >= 1 && n <= max
  const changed = valid && n !== service.instances
  const set = (next: number) => {
    setError(null)
    setValue(String(Math.max(1, Math.min(max, next))))
  }

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!changed) return
    setError(null)
    // mutateAsync: the success toast must survive this form re-rendering with the new count
    scale.mutateAsync(n).then(
      () => {
        toast.success(`${service.name} scaled to ${plural(n, 'instance')}`)
        onDone()
      },
      (err: unknown) => setError(errorMessage(err)),
    )
  }

  return (
    <form onSubmit={submit} noValidate className="flex min-h-0 flex-col">
      <DialogBody>
        <div className="flex flex-col gap-2">
          <label htmlFor={inputId} className="text-sm font-medium text-foreground">
            Instances
          </label>
          <div className="flex items-center gap-2">
            <Button
              size="icon-md"
              icon={<Minus />}
              aria-label="One instance less"
              onClick={() => set((Number.isFinite(n) ? n : 1) - 1)}
              disabled={!Number.isFinite(n) || n <= 1}
            />
            <Input
              id={inputId}
              type="number"
              inputMode="numeric"
              min={1}
              max={max}
              step={1}
              value={value}
              onChange={(e) => {
                setError(null)
                setValue(e.target.value)
              }}
              aria-invalid={!valid || undefined}
              aria-describedby={hintId}
              className="w-24 text-center tabular"
              autoFocus
            />
            <Button
              size="icon-md"
              icon={<Plus />}
              aria-label="One more instance"
              onClick={() => set((Number.isFinite(n) ? n : 0) + 1)}
              disabled={!Number.isFinite(n) || n >= max}
            />
          </div>
          <p id={hintId} className={valid ? 'text-[13px] text-foreground-light' : 'text-[13px] text-destructive'}>
            {valid
              ? service.disk_mount_path
                ? 'Services with a persistent disk are limited to 1 instance.'
                : `Between 1 and ${MAX_INSTANCES}. Currently ${plural(service.instances, 'instance')}.`
              : `Enter a whole number between 1 and ${max}.`}
          </p>
        </div>
        {error && (
          <p
            role="alert"
            className="rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] text-foreground"
          >
            {error}
          </p>
        )}
      </DialogBody>
      <DialogFooter>
        <Button onClick={onDone} disabled={scale.isPending}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={!changed} loading={scale.isPending}>
          {changed ? `Scale to ${plural(n, 'instance')}` : 'Scale'}
        </Button>
      </DialogFooter>
    </form>
  )
}

/** Change the number of instances (`POST /services/{id}/scale`). */
export function ScaleDialog({ service, open, onOpenChange }: ScaleDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="sm">
        <DialogHeader>
          <DialogTitle>Scale {service.name}</DialogTitle>
          <DialogDescription>
            {isPublicHttp(service.type)
              ? 'Each instance is its own container; the proxy balances requests across them.'
              : 'Each instance is its own container running the same start command.'}
          </DialogDescription>
        </DialogHeader>
        {/* mounted per opening, so the field starts from the current count */}
        {open && <ScaleForm service={service} onDone={() => onOpenChange(false)} />}
      </DialogContent>
    </Dialog>
  )
}
