import { AlertTriangle, RotateCw } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Hint } from '@/components/ui/tooltip'
import { plural } from '@/lib/format'

interface VariablesFooterProps {
  dirty: boolean
  valid: boolean
  /** Saved variables (shown when there is nothing to save). */
  count: number
  /** Last save error (API message). */
  error?: string | null
  /** Offer "Save & restart" (something live would pick the change up). */
  canRestart: boolean
  /** Tooltip of "Save & restart" (what gets restarted). */
  restartHint?: string
  saving: 'save' | 'restart' | null
  onCancel: () => void
  onSaveAndRestart: () => void
}

/**
 * Footer of every variable editor (service Environment, env group):
 * status on the left, then `Cancel · Save only · [Save & restart]` with the
 * restart as the primary action, or `Cancel · [Save changes]` when nothing
 * would restart. "Save" is the form's submit button (Enter in a field saves
 * without restarting).
 */
export function VariablesFooter({
  dirty,
  valid,
  count,
  error,
  canRestart,
  restartHint,
  saving,
  onCancel,
  onSaveAndRestart,
}: VariablesFooterProps) {
  const busy = saving !== null
  const restart = (
    <Button
      type="button"
      variant="primary"
      icon={<RotateCw />}
      disabled={!dirty || !valid || (busy && saving !== 'restart')}
      loading={saving === 'restart'}
      onClick={onSaveAndRestart}
    >
      Save & restart
    </Button>
  )
  return (
    <>
      <div className="mr-auto flex min-w-0 items-center gap-2 text-[13px] text-foreground-light">
        {error ? (
          <p role="alert" className="flex min-w-0 items-start gap-1.5 text-destructive">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            <span className="break-words">{error}</span>
          </p>
        ) : dirty ? (
          <>
            <span aria-hidden="true" className="size-1.5 shrink-0 rounded-full bg-warning" />
            <span>
              Unsaved changes
              {!valid && <span className="text-destructive"> · fix the highlighted rows</span>}
            </span>
          </>
        ) : (
          <span>{plural(count, 'variable')}</span>
        )}
      </div>
      <Button type="button" disabled={!dirty || busy} onClick={onCancel}>
        Cancel
      </Button>
      <Button
        type="submit"
        variant={canRestart ? 'default' : 'primary'}
        disabled={!dirty || !valid || (busy && saving !== 'save')}
        loading={saving === 'save'}
      >
        {canRestart ? 'Save only' : 'Save changes'}
      </Button>
      {canRestart && (restartHint ? <Hint label={restartHint}>{restart}</Hint> : restart)}
    </>
  )
}
