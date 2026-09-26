import * as React from 'react'
import { AlertTriangle } from 'lucide-react'

import {
  AlertDialog,
  AlertDialogBody,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { errorMessage } from '@/lib/api/client'
import { cn } from '@/lib/utils'

export interface ConfirmDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: React.ReactNode
  description?: React.ReactNode
  /** Label of the confirm button. */
  confirmLabel?: string
  /** `danger` = solid red confirm (default), `primary` = green, `warning` = amber. */
  variant?: 'danger' | 'primary' | 'warning'
  /**
   * Typed confirmation: the user must type this exact text (usually the
   * resource name) before the confirm button enables.
   */
  confirmText?: string
  /**
   * Called on confirm. If it returns a promise the dialog shows a spinner,
   * closes when it resolves and shows the error message when it rejects.
   */
  onConfirm: () => unknown
  /** Extra content (checkboxes such as "Force delete", warnings…). */
  children?: React.ReactNode
}

/** Confirmation for destructive / impactful actions, with optional typed-name check. */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel = 'Confirm',
  variant = 'danger',
  confirmText,
  onConfirm,
  children,
}: ConfirmDialogProps) {
  const [typed, setTyped] = React.useState('')
  const [pending, setPending] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const inputId = React.useId()

  const matches = confirmText === undefined || typed === confirmText
  const run = async () => {
    if (!matches || pending) return
    setError(null)
    try {
      const result = onConfirm()
      if (result instanceof Promise) {
        setPending(true)
        await result
      }
      onOpenChange(false)
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setPending(false)
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={(o) => !pending && onOpenChange(o)}>
      <AlertDialogContent
        onOpenAutoFocus={() => {
          // fresh state every time the dialog opens
          setTyped('')
          setError(null)
          setPending(false)
        }}
      >
        <form
          onSubmit={(e) => {
            e.preventDefault()
            void run()
          }}
          className="flex min-h-0 flex-col"
        >
          <AlertDialogHeader>
            <AlertDialogTitle>{title}</AlertDialogTitle>
          </AlertDialogHeader>
          <AlertDialogBody>
            {description && (
              <AlertDialogDescription asChild>
                <div className="flex flex-col gap-2">{description}</div>
              </AlertDialogDescription>
            )}
            {children}
            {confirmText !== undefined && (
              <div className="flex flex-col gap-2">
                <label htmlFor={inputId} className="text-[13px] text-foreground-light">
                  Type <span className="font-mono font-medium text-foreground select-all">{confirmText}</span> to confirm.
                </label>
                <Input
                  id={inputId}
                  mono
                  autoComplete="off"
                  spellCheck={false}
                  autoFocus
                  value={typed}
                  onChange={(e) => setTyped(e.target.value)}
                  placeholder={confirmText}
                />
              </div>
            )}
            {error && (
              <div
                role="alert"
                className="flex items-start gap-2 rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] text-foreground"
              >
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
                <span className="break-words">{error}</span>
              </div>
            )}
          </AlertDialogBody>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
            <Button
              type="submit"
              variant={variant === 'danger' ? 'danger-solid' : variant === 'warning' ? 'warning' : 'primary'}
              disabled={!matches}
              loading={pending}
              className={cn(!matches && 'opacity-50')}
            >
              {confirmLabel}
            </Button>
          </AlertDialogFooter>
        </form>
      </AlertDialogContent>
    </AlertDialog>
  )
}
